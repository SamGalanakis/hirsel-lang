//! A real native waiting take, entered cut, later acquisition and gap replay.
use super::*;
use crate::tasks::free::record;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use hirsel_free::session::Session;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct Child {
    handle: u64,
    finished: AtomicBool,
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
