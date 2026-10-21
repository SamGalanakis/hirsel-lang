use super::*;
use crate::tasks::free::record;
use hirsel_free::projection::Projection;
use hirsel_free::session::Session;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Race {
    checked: AtomicBool,
    canceled: AtomicBool,
    finished: AtomicBool,
}

fn wait_until(mut done: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !done() {
        assert!(
            std::time::Instant::now() < until,
            "native cancellation must advance"
        );
        std::thread::yield_now();
    }
}

fn check_value(ctx: *mut TaskCtx, expected: u32) {
    let mut value = 99;
    unsafe { hirsel_rt_v2_task_canceled(ctx, &mut value, &0) };
    assert_eq!(value, expected);
}

unsafe extern "C" fn child(ctx: *mut u8, data: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let race = unsafe { Box::from_raw(data.cast::<Arc<Race>>()) };
    check_value(ctx, 0);
    race.checked.store(true, Ordering::Release);
    wait_until(|| race.canceled.load(Ordering::Acquire));
    check_value(ctx, 1 + CANCEL_REQUESTED);
    unsafe {
        let _role = role::Guard::new(ctx);
        (*ctx).shield = 1;
    }
    check_value(ctx, 0);
    unsafe {
        let _role = role::Guard::new(ctx);
        (*ctx).shield = 0;
    }
    check_value(ctx, 1 + CANCEL_REQUESTED);
    race.finished.store(true, Ordering::Release);
    drop(race);
    unsafe {
        role::enter(ctx);
        crate::pool::free::finish_current(ctx);
    }
}

#[test]
fn native_cancel_and_unshielded_checks_record_observations_and_replay_the_cut() {
    const CHILD: &str = "HIRSEL_F1_CANCEL_RECORDING";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::cancellation::tests::native_cancel_and_unshielded_checks_record_observations_and_replay_the_cut", "--nocapture"])
            .env(CHILD, "1")
            .output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(2);
    for durable in [false, true] {
        let recording = record::Recording::new(b"native cancellation", durable);
        let install = recording.clone();
        let cutter = recording.clone();
        let race = Arc::new(Race::default());
        let mut run = crate::host::BodyRun::new(2, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| {
                let mut scope_word = 0;
                unsafe {
                    hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut scope_word, &0);
                }
                let word = unsafe {
                    let _role = role::Guard::new(ctx);
                    let stack = Stack::new();
                    let child_ctx = stack.context();
                    child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
                    let log = record::child(ctx).unwrap();
                    (*child_ctx).recording = Some(Box::new(log));
                    let s = scheduler(ctx);
                    let scope = s.scope_handle(scope_word);
                    let id = s.tasks.len() as u32;
                    (*child_ctx).task_id = id;
                    (*child_ctx).task_sp = stack.prepare(
                        child,
                        child_ctx.cast(),
                        Box::into_raw(Box::new(race.clone())).cast(),
                    );
                    let mut row = Task::new(1, child_ctx, Some(scope), vec![0], None);
                    row.stack = Some(stack);
                    s.tasks.push(Some(row));
                    s.scope(scope).children.push(id);
                    s.link_child(ROOT, id);
                    crate::pool::free::schedule_saved((*ctx).run_ptr(), id, Owner::Any);
                    handle(id, 1)
                };
                wait_until(|| race.checked.load(Ordering::Acquire));
                cutter.recorder.flush().unwrap().unwrap();
                unsafe {
                    hirsel_rt_v2_task_cancel(ctx, &scope_word, &word);
                    // The first cause stays Requested across a repeated cancel.
                    hirsel_rt_v2_task_cancel(ctx, &scope_word, &word);
                }
                race.canceled.store(true, Ordering::Release);
                wait_until(|| race.finished.load(Ordering::Acquire));
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        recording.recorder.stop("returned", None).unwrap().unwrap();
        let trace = recording.trace();
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
            let terminal = end == marked.bytes.len();
            let mut replay = Session::replay(bytes).unwrap();
            assert!(replay.start(&[]));
            assert!(replay.start(&[0]));
            for _ in 0..if terminal { 4 } else { 2 } {
                replay.begin(&[]);
                replay.publish(&[], Phase::Done).unwrap();
            }
            for op in 1..=if terminal { 4 } else { 1 } {
                replay.begin(&[0]);
                if op != 3 {
                    assert_eq!(
                        replay.cancel_check(&[0], None).unwrap(),
                        (op != 1).then_some(CANCEL_REQUESTED)
                    );
                }
                replay.publish(&[0], Phase::Done).unwrap();
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
