use super::*;
use crate::tasks::free::record;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use hirsel_free::session::Session;
use std::sync::{Arc, Mutex};

struct Trace {
    order: Mutex<Vec<u64>>,
    recording: Arc<record::Recording>,
    cancel: bool,
}

unsafe extern "C" fn child(ctx: *mut TaskCtx, environment: *mut Header, _: u64, _: u64) -> u8 {
    let (id, trace) = unsafe { environment.add(1).cast::<(u64, Arc<Trace>)>().read() };
    let mut image = [1u64, id];
    let none = [0u64; 2];
    unsafe { hirsel_rt_v2_task_finish(ctx, image.as_mut_ptr().cast(), none.as_ptr().cast(), 16) };
    assert_eq!(
        image,
        [1, id],
        "a discarded result stays on its owner's frame"
    );
    let cut = trace.recording.recorder.flush().unwrap().unwrap();
    assert_eq!(cut.progress[&Vec::new()], Progress::new(9, Phase::Entered));
    let cause = unsafe {
        let _role = role::Guard::new(ctx);
        canceled(ctx)
    };
    assert_eq!(cause, trace.cancel.then_some(CANCEL_REQUESTED));
    trace.order.lock().unwrap().push(id);
    STATUS_OK
}

#[test]
fn bulk_release_calls_saved_rests_in_add_order_under_one_recorded_ordinal() {
    for width in [2, 8] {
        for durable in [false, true] {
            for cancel in [false, true] {
                run(width, durable, cancel);
            }
        }
    }
}

fn run(width: u32, durable: bool, cancel: bool) {
    let recording = record::Recording::new(b"native bulk release", durable);
    let install = recording.clone();
    let trace = Arc::new(Trace {
        order: Mutex::new(Vec::new()),
        recording: recording.clone(),
        cancel,
    });
    let watched = trace.clone();
    let layout = crate::Layout {
        magic: crate::LAYOUT_MAGIC,
        version: crate::LAYOUT_VERSION,
        size: std::mem::size_of::<(u64, Arc<Trace>)>() as u64,
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
    let mut run = crate::host::BodyRun::new(width, false);
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
            let mut scope = 0;
            hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
            let mut set = 0;
            hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope);
            let mut children = [0u64; 3];
            for (id, word) in children.iter_mut().enumerate() {
                let env = crate::alloc::hirsel_rt_v2_alloc(
                    ctx,
                    2,
                    layout.size,
                    8,
                    crate::alloc::DOMAIN_HEAP,
                    0,
                );
                env.add(1)
                    .cast::<(u64, Arc<Trace>)>()
                    .write((id as u64, trace.clone()));
                hirsel_rt_v2_task_spawn(ctx, 0, 0, std::ptr::null_mut(), word, &scope, child, env);
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                loop {
                    let held = {
                        let _role = role::Guard::new(ctx);
                        let s = scheduler(ctx);
                        s.task(*word as u32).state == State::Holding
                            && readiness::saved(s, *word as u32)
                    };
                    if held {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < until,
                        "child reaches saved Holding"
                    );
                    std::thread::yield_now();
                }
            }
            for id in [2, 0, 1] {
                hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut children[id]);
            }
            if cancel {
                assert_eq!(
                    hirsel_rt_v2_task_cancel_all(ctx, 0, 0, &scope, &mut set),
                    STATUS_OK
                );
            } else {
                assert_eq!(
                    hirsel_rt_v2_task_detach_all(ctx, &scope, &mut set),
                    STATUS_OK
                );
            }
            {
                let _role = role::Guard::new(ctx);
                assert!(scheduler(ctx).scope(scope as u32).children.is_empty());
                assert!(scheduler(ctx).sets[set as u32 as usize].is_none());
            }
            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0)
        },
    );
    assert_eq!(outcome.ended, Ok(STATUS_OK));
    assert_eq!(outcome.pending, 0);
    assert_eq!(*watched.order.lock().unwrap(), [2, 0, 1]);
    drop(watched);
    let cut = recording
        .recorder
        .stop("returned", None)
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(cut.progress[&Vec::new()], Progress::new(10, Phase::Done));
    let trace = recording.trace();
    let marked = Arc::try_unwrap(recording)
        .ok()
        .unwrap()
        .recorder
        .into_sink();
    for end in marked.marks {
        let bytes = &marked.bytes[..end];
        let tape = hirsel_free::tape::parse(bytes).unwrap();
        let mut replay = Session::replay(bytes).unwrap();
        if !tape.cut.is_empty() {
            assert!(replay.start(&[]));
            for id in 0..3 {
                assert!(replay.start(&[id]));
                replay.begin(&[id]);
                replay.publish(&[id], Phase::Entered).unwrap();
                replay.publish(&[id], Phase::Done).unwrap();
            }
            for op in 1..=tape.cut[&Vec::new()].op {
                replay.begin(&[]);
                if op >= 9 {
                    replay.publish(&[], Phase::Entered).unwrap();
                }
                if op < tape.cut[&Vec::new()].op || tape.cut[&Vec::new()].phase == Phase::Done {
                    replay.publish(&[], Phase::Done).unwrap();
                }
            }
        } else {
            assert!(!replay.start(&[]));
        }
        assert_eq!(
            replay.finish().unwrap(),
            Projection::new(
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

struct Waiting {
    receiver: u64,
    recording: Arc<record::Recording>,
    observed: std::sync::atomic::AtomicU32,
}

unsafe extern "C" fn waiting(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let shared = unsafe { env.add(1).cast::<Arc<Waiting>>().read() };
    let mut receiver = shared.receiver;
    let mut image = [0u64; 2];
    let mut status = 99;
    unsafe {
        hirsel_rt_v2_channel_receive(
            ctx,
            0,
            0,
            &mut status,
            &mut receiver,
            image.as_mut_ptr().cast(),
        );
    }
    assert_eq!(status, crate::channels::CANCELED + CANCEL_REQUESTED);
    assert_eq!(image, [0, 0]);
    let cut = shared.recording.recorder.flush().unwrap().unwrap();
    assert_eq!(cut.progress[&Vec::new()], Progress::new(6, Phase::Entered));
    shared.observed.store(status, Ordering::Release);
    STATUS_OK
}

#[test]
fn cancel_all_drains_a_saved_receiver_before_done_and_closes_an_empty_set() {
    for width in [2, 8] {
        for durable in [false, true] {
            let recording = record::Recording::new(b"native cancel all drain", durable);
            let install = recording.clone();
            let own = recording.clone();
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
            let mut run = crate::host::BodyRun::new(width, false);
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
                    let mut scope = 0;
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope, &0);
                    let mut endpoints = [0u64; 2];
                    let none = [0u64; 2];
                    hirsel_rt_v2_channel_open(
                        ctx,
                        std::ptr::null_mut(),
                        endpoints.as_mut_ptr(),
                        &0,
                        none.as_ptr().cast(),
                        2,
                    );
                    let mut set = 0;
                    hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope);
                    let shared = Arc::new(Waiting {
                        receiver: endpoints[1],
                        recording: own.clone(),
                        observed: std::sync::atomic::AtomicU32::new(99),
                    });
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        16,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    env.add(1).cast::<Arc<Waiting>>().write(shared.clone());
                    let mut child = 0;
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        waiting,
                        env,
                    );
                    hirsel_rt_v2_task_add(ctx, &scope, &mut set, &mut child);
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    loop {
                        let parked = {
                            let _role = role::Guard::new(ctx);
                            let s = scheduler(ctx);
                            s.task(child as u32).state == State::Parked
                                && readiness::saved(s, child as u32)
                        };
                        if parked {
                            break;
                        }
                        assert!(
                            std::time::Instant::now() < until,
                            "receiver must reach idle"
                        );
                        std::thread::yield_now();
                    }
                    assert_eq!(
                        hirsel_rt_v2_task_cancel_all(ctx, 0, 0, &scope, &mut set),
                        STATUS_OK
                    );
                    assert_eq!(
                        shared.observed.load(Ordering::Acquire),
                        crate::channels::CANCELED + CANCEL_REQUESTED
                    );
                    {
                        let _role = role::Guard::new(ctx);
                        let s = scheduler(ctx);
                        assert!(s.member(child).is_none());
                        assert!(s.sets[set as u32 as usize].is_none());
                    }
                    drop(shared);
                    // Bulk release of an empty set still consumes exactly one
                    // semantic ordinal and recycles the newly reserved row.
                    hirsel_rt_v2_task_set(ctx, std::ptr::null_mut(), &mut set, &scope);
                    hirsel_rt_v2_task_detach_all(ctx, &scope, &mut set);
                    for endpoint in endpoints {
                        let _role = role::Guard::new(ctx);
                        assert_eq!(close_endpoint(ctx, endpoint, 0, 0), STATUS_OK);
                    }
                    hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0)
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
            assert_eq!(cut.progress[&Vec::new()], Progress::new(11, Phase::Done));
            assert_eq!(cut.progress[&vec![0]], Progress::new(2, Phase::Done));
        }
    }
}
