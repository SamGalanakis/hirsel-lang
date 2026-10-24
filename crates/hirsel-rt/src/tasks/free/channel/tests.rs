use super::*;
use crate::tasks::free::record;
use crate::tasks::free::replay::Replay;
use hirsel_free::projection::{Event, Projection};
use hirsel_free::session::Session;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const OPS: u64 = 32;

pub(super) fn layout() -> crate::Layout {
    crate::Layout {
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
    }
}

struct Child {
    sender: u64,
    id: u64,
    race: Arc<Race>,
}

#[derive(Default)]
struct Race {
    started: AtomicU32,
    middle: AtomicU32,
    resume: AtomicBool,
    finished: AtomicU32,
}

pub(super) fn wait_until(mut done: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !done() {
        assert!(
            std::time::Instant::now() < until,
            "native senders must advance"
        );
        std::thread::yield_now();
    }
}

fn exercise(
    ctx: *mut TaskCtx,
    sender: u64,
    id: u64,
    race: &Race,
    recording: Option<&record::Recording>,
) {
    race.started.fetch_add(1, Ordering::Release);
    wait_until(|| race.started.load(Ordering::Acquire) == 8);
    for op in 0..OPS {
        let mut image = [1u64, id * 1000 + op];
        let mut sender = sender;
        let mut status = 99;
        unsafe {
            hirsel_rt_v2_channel_send(ctx, &mut status, &mut sender, image.as_mut_ptr().cast());
        }
        assert_eq!(status, crate::channels::DONE);
        assert_eq!(image, [0, 0]);
        if op + 1 == OPS / 2 {
            race.middle.fetch_add(1, Ordering::Release);
            if let Some(recording) = recording {
                wait_until(|| race.middle.load(Ordering::Acquire) == 8);
                let cut = recording.recorder.flush().unwrap().unwrap();
                for (path, progress) in cut.progress {
                    assert_eq!(progress.op, OPS / 2 + if path.is_empty() { 15 } else { 0 });
                }
                race.resume.store(true, Ordering::Release);
            } else {
                wait_until(|| race.resume.load(Ordering::Acquire));
            }
        }
    }
}

unsafe extern "C" fn child(ctx: *mut u8, data: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let child = unsafe { Box::from_raw(data.cast::<Child>()) };
    exercise(ctx, child.sender, child.id, &child.race, None);
    child.race.finished.fetch_add(1, Ordering::Release);
    drop(child);
    unsafe {
        role::enter(ctx);
        crate::pool::free::finish_current(ctx);
    }
}

#[test]
fn native_sends_keep_images_and_gap_projections_in_the_same_fifo_order() {
    const CHILD: &str = "HIRSEL_F1_CHANNEL_RECORDING";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::channel::tests::native_sends_keep_images_and_gap_projections_in_the_same_fifo_order", "--nocapture"])
            .env(CHILD, "1")
            .output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(8);
    for durable in [false, true] {
        let recording = record::Recording::new(b"native concurrent channel", durable);
        let install = recording.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let image_layout = layout();
        let layouts = [&raw const image_layout];
        run.config.layouts = layouts.as_ptr();
        run.config.layout_count = 1;
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_out = received.clone();
        let flush_recording = recording.clone();
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| {
                let mut endpoints = [0u64; 2];
                let none = [0u64; 2];
                unsafe {
                    hirsel_rt_v2_channel_open(
                        ctx,
                        std::ptr::null_mut(),
                        endpoints.as_mut_ptr(),
                        &0,
                        none.as_ptr().cast(),
                        2,
                    );
                }
                let race = Arc::new(Race::default());
                for index in 0..7 {
                    let mut sender = 0;
                    unsafe {
                        hirsel_rt_v2_sender_dup(
                            ctx,
                            std::ptr::null_mut(),
                            &mut sender,
                            &0,
                            &endpoints[0],
                        );
                        let _role = role::Guard::new(ctx);
                        let stack = Stack::new();
                        let child_ctx = stack.context();
                        child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
                        let log = record::child(ctx).unwrap();
                        let path = log.path().to_vec();
                        (*child_ctx).recording = Some(Box::new(log));
                        let s = scheduler(ctx);
                        let id = s.tasks.len() as u32;
                        (*child_ctx).task_id = id;
                        (*child_ctx).task_sp = stack.prepare(
                            child,
                            child_ctx.cast(),
                            Box::into_raw(Box::new(Child {
                                sender,
                                id: index + 1,
                                race: race.clone(),
                            }))
                            .cast(),
                        );
                        let mut row = Task::new(1, child_ctx, None, path, None);
                        row.stack = Some(stack);
                        s.tasks.push(Some(row));
                        crate::pool::free::schedule_saved((*ctx).run_ptr(), id, Owner::Any);
                    }
                }
                // Cancellation must neither reject sends nor add a check to
                // their tapes. Receive's cancellation is a separate boundary.
                unsafe {
                    let _role = role::Guard::new(ctx);
                    scheduler(ctx).cancel(ROOT, CANCEL_REQUESTED);
                }
                exercise(ctx, endpoints[0], 0, &race, Some(&flush_recording));
                wait_until(|| race.finished.load(Ordering::Acquire) == 7);
                let mut received = received_out.lock().unwrap();
                unsafe {
                    let _role = role::Guard::new(ctx);
                    let s = scheduler(ctx);
                    let channel = s.channels.channel(endpoints[1], false);
                    for _ in 0..8 * OPS {
                        let mut image = [0u64; 2];
                        assert_eq!(
                            s.channels.receive(channel, image.as_mut_ptr().cast()),
                            Ok(true)
                        );
                        assert_eq!(image[0], 1);
                        received.push(image[1]);
                    }
                    assert!(!s.channels.ready(channel));
                }
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
        let trace = recording.trace();
        let projection = Projection::new("returned", &cut.progress, &trace);
        assert!(
            projection.observed.is_empty(),
            "send has no cancellation check"
        );
        let history = projection.objects.values().next().unwrap();
        assert_eq!(history.versions.len(), 1 + 7 + 8 * OPS as usize);
        let expected: Vec<_> = history
            .versions
            .values()
            .filter(|touch| touch.outcome == "send")
            .map(|touch| {
                if touch.task.is_empty() {
                    touch.op - 16
                } else {
                    u64::from(touch.task[0] + 1) * 1000 + touch.op - 1
                }
            })
            .collect();
        assert_eq!(*received.lock().unwrap(), expected);
        let marked = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink();
        for end in marked.marks {
            let bytes = &marked.bytes[..end];
            let tape = hirsel_free::tape::parse(bytes).unwrap();
            if tape.cut.is_empty() {
                continue;
            }
            let cut = &tape.cut;
            let mut replay = Session::replay(bytes).unwrap();
            let paths: Vec<Vec<u32>> = std::iter::once(Vec::new())
                .chain((0..7).map(|id| vec![id]))
                .collect();
            for path in &paths {
                assert!(replay.start(path));
            }
            let handle = (crate::free::CHANNEL, 1);
            let mut next = [1u64; 8];
            let mut begun = [false; 8];
            loop {
                let mut advanced = false;
                let mut pending = false;
                for (index, path) in paths.iter().enumerate() {
                    if next[index] > cut[path].op {
                        continue;
                    }
                    pending = true;
                    if !begun[index] {
                        assert_eq!(replay.begin(path), next[index]);
                        begun[index] = true;
                    }
                    let op = next[index];
                    if index == 0 && op == 1 {
                        replay.create(path, handle).unwrap();
                    } else if index != 0 || op >= 16 || op % 2 == 0 {
                        if !replay.enabled(path, 0, handle).unwrap() {
                            continue;
                        }
                        replay
                            .touch(
                                path,
                                0,
                                handle,
                                if index == 0 && op < 16 { "dup" } else { "send" },
                            )
                            .unwrap();
                    }
                    replay.publish(path, Phase::Done).unwrap();
                    next[index] += 1;
                    begun[index] = false;
                    advanced = true;
                }
                if !pending {
                    break;
                }
                assert!(advanced, "gap replay must make progress");
            }
            assert_eq!(
                replay.finish().unwrap(),
                Projection::new(
                    if end == marked.bytes.len() {
                        "returned"
                    } else {
                        "truncated"
                    },
                    cut,
                    &trace
                )
            );
        }
        assert_eq!(
            trace
                .iter()
                .filter(|s| matches!(s.event, Event::Versioned { .. }))
                .count(),
            history.versions.len()
        );
    }
}

#[test]
fn a_canceled_native_sender_keeps_its_payload_when_the_receiver_is_closed() {
    let mut run = crate::host::BodyRun::new(2, false);
    run.free = true;
    unsafe { run.use_physical_dispatch() };
    let image_layout = layout();
    let layouts = [&raw const image_layout];
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    let outcome = run.run(
        |ctx| unsafe { super::super::enable(ctx) },
        |ctx| {
            let mut endpoints = [0u64; 2];
            let none = [0u64; 2];
            unsafe {
                hirsel_rt_v2_channel_open(
                    ctx,
                    std::ptr::null_mut(),
                    endpoints.as_mut_ptr(),
                    &0,
                    none.as_ptr().cast(),
                    2,
                );
                let _role = role::Guard::new(ctx);
                assert_eq!(close_endpoint(ctx, endpoints[1], 0, 0), STATUS_OK);
                scheduler(ctx).cancel(ROOT, CANCEL_REQUESTED);
            }
            let mut image = [1u64, 99];
            let mut status = 99;
            unsafe {
                hirsel_rt_v2_channel_send(
                    ctx,
                    &mut status,
                    &mut endpoints[0],
                    image.as_mut_ptr().cast(),
                );
            }
            assert_eq!(status, crate::channels::CLOSED);
            assert_eq!(image, [1, 99]);
            STATUS_OK
        },
    );
    assert_eq!(outcome.ended, Ok(STATUS_OK));
    assert_eq!(outcome.pending, 0);
}
