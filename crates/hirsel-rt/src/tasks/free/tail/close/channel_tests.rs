use super::*;
use crate::tasks::free::objects::tests::{isolated_at, plain, wait_until};
use crate::tasks::*;
use std::sync::{Arc, atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering}};

static ORDER: AtomicU64 = AtomicU64::new(0);
static FAILURES: AtomicBool = AtomicBool::new(false);
unsafe extern "C" fn leaf(ctx: *mut TaskCtx, value: *mut u8, _: u64, _: u64) -> u8 {
    let [tag, id] = unsafe { value.cast::<[u64; 2]>().read() };
    assert_eq!(tag, 1);
    ORDER.fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| Some(old * 10 + id)).unwrap();
    if FAILURES.load(Ordering::Acquire) && matches!(id, 2 | 4) {
        unsafe { crate::failure::hirsel_rt_v2_failure_begin(ctx, 1, (900 + id) as u32, 31, 41) };
        return crate::STATUS_FAILURE;
    }
    STATUS_OK
}
struct Shared {
    receiver: u64,
    blocked: u32,
    task: AtomicU32,
    polls: AtomicU32,
    ordinal: AtomicU64,
    saved: AtomicBool,
    go: AtomicBool,
}
unsafe extern "C" fn dispatch(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64, frame: *mut u8) -> u8 {
    unsafe {
        let shared = &**crate::alloc::payload(env).cast::<*const Shared>();
        let result = frame.add(24).cast::<u64>();
        if frame.cast::<u32>().read() == 2 {
            return hirsel_rt_v2_task_finish(ctx, result.cast(), [0u64; 2].as_ptr().cast(), 16, u32::MAX);
        }
        let _role = role::Guard::new(ctx);
        crate::tasks::compact::waited(ctx);
        if frame.cast::<u32>().read() == 0 {
            shared.task.store((*ctx).task_id, Ordering::Release);
            frame.add(16).cast::<u64>().write(shared.receiver);
            let mut inline = Box::new(std::mem::MaybeUninit::<Cleanup>::uninit());
            let item = crate::finalize::finalize_item(inline.as_mut_ptr(), 4, frame.add(16));
            let cursor = Box::new(Cursor::new((*ctx).work, item, ptr::null(), false, 31, 41));
            let index = attach(ctx, cursor, 31, 41);
            let Node::Release { _inline, .. } = &mut *(&mut (*ctx).free_ops.tails.drivers)[index].stack[0] else { unreachable!() };
            *_inline = Some(inline);
            crate::release::push(&mut *ctx, item, true);
            frame.add(8).cast::<usize>().write(index);
            frame.cast::<u32>().write(1);
        }
        let index = frame.add(8).cast::<usize>().read();
        let driver = &mut *(&mut (*ctx).free_ops.tails.drivers)[index] as *mut Driver;
        let polled = (*driver).poll_with(ctx, |object, entered| {
            if !*entered {
                record::publish(ctx, hirsel_free::phase::Phase::Entered, |_| ())?;
                *entered = true;
            }
            if object == (crate::free::CHANNEL, u64::from(shared.blocked)) {
                let ordinal = record::position(ctx).1.op;
                let previous = shared.ordinal.compare_exchange(0, ordinal, Ordering::AcqRel, Ordering::Acquire).unwrap_or_else(|old| old);
                assert!(previous == 0 || previous == ordinal);
                shared.polls.fetch_add(1, Ordering::Release);
                Ok(shared.go.load(Ordering::Acquire))
            } else { Ok(true) }
        }).unwrap();
        if polled.is_pending() {
            assert_eq!((*ctx).shield, 2);
            assert_eq!((*ctx).detached_images.len(), 1);
            assert_eq!(ORDER.load(Ordering::Acquire), 0, "no FIFO successor before blocked inner close");
            crate::tasks::park_left(ctx, Wait::Retry { cancelable: false }).unwrap();
            crate::tasks::compact::wait(ctx);
            return crate::STATUS_SUSPENDED;
        }
        let expected = if FAILURES.load(Ordering::Acquire) { crate::STATUS_FAILURE } else { STATUS_OK };
        assert_eq!(polled, Poll::Ready(expected));
        if expected == crate::STATUS_FAILURE {
            assert_eq!((*ctx).failure.chain(&[]).iter().map(|(record,_)| record.rule).collect::<Vec<_>>(), [902, 904]);
            crate::failure::hirsel_rt_v2_failure_clear(ctx);
        }
        (*ctx).free_ops.tails.drivers.pop();
        assert_eq!((*ctx).shield, 0);
        assert!((*ctx).work.is_null());
        assert_eq!((*ctx).detached_images.len(), 0);
        assert_eq!(ORDER.load(Ordering::Acquire), 1234);
        result.write(1); result.add(1).write(1234);
        frame.cast::<u32>().write(2);
        hirsel_rt_v2_task_finish(ctx, result.cast(), [0u64; 2].as_ptr().cast(), 16, u32::MAX)
    }
}
unsafe extern "C" fn child(ctx: *mut TaskCtx, env: *mut Header, caller: u64, site: u64) -> u8 {
    unsafe { crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, dispatch, 48) }
}
#[test]
fn nested_receiver_close_keeps_fifo_ancestry_off_stack_across_duplicate_wakes() {
    run_nested("tasks::free::tail::close::channel_tests::nested_receiver_close_keeps_fifo_ancestry_off_stack_across_duplicate_wakes", "HIRSEL_TAIL_NESTED_RECEIVER", false, false);
}
#[test]
fn nested_receiver_failures_drain_all_fifo_siblings_and_preserve_failure_order() {
    run_nested("tasks::free::tail::close::channel_tests::nested_receiver_failures_drain_all_fifo_siblings_and_preserve_failure_order", "HIRSEL_TAIL_NESTED_FAILURES", true, false);
}
#[test]
fn stopping_saved_nested_receiver_reclaims_custody_without_language_cleanup() {
    run_nested("tasks::free::tail::close::channel_tests::stopping_saved_nested_receiver_reclaims_custody_without_language_cleanup", "HIRSEL_TAIL_NESTED_STOP", false, true);
}
fn run_nested(test: &str, variable: &str, failures: bool, stopping: bool) {
    if !isolated_at(test, variable) { return; }
    for width in [1, 4] { for durable in [false, true] {
        ORDER.store(0, Ordering::Release);
        FAILURES.store(failures, Ordering::Release);
        let leaf = crate::tasks::free::finalized(crate::Layout { finalizer: Some(leaf), ..plain(16) });
        let close = Box::leak(Box::new(crate::Step { kind: crate::STEP_CLOSE_HANDLE, offset: 0, field: crate::WHOLE_VALUE, variant: 0, span: 0, stride: 0, count: 0, count_offset: crate::NO_OFFSET, layout: ptr::null() }));
        let endpoint = crate::Layout { steps: close, step_count: 1, ..plain(8) };
        let image_step = Box::leak(Box::new(crate::Step { offset: 8, ..*close }));
        let image = crate::Layout { steps: image_step, step_count: 1, ..plain(16) };
        let env = plain(8);
        let layouts = [leaf as *const crate::Layout, &image, &endpoint, &env];
        let mut run = crate::host::BodyRun::new(width, false);
        run.free = true; run.config.layouts = layouts.as_ptr(); run.config.layout_count = 4;
        unsafe { run.use_physical_dispatch() };
        let recording = record::Recording::new(b"nested receiver FIFO", durable);
        let install = recording.clone();
        let control = run.control();
        let stopper = control.clone();
        let outcome = run.run(move |ctx| unsafe { crate::tasks::free::enable(ctx); install.root(ctx) }, move |ctx| unsafe {
            let none = [0u64; 2];
            let mut outer = [0u64; 2];
            hirsel_rt_v2_channel_open(ctx, ptr::null_mut(), outer.as_mut_ptr(), &0, none.as_ptr().cast(), 3);
            let mut blocked = 0;
            for group in 0..2 {
                let mut inner = [0u64; 2];
                hirsel_rt_v2_channel_open(ctx, ptr::null_mut(), inner.as_mut_ptr(), &0, none.as_ptr().cast(), 2);
                if group == 0 {
                    let _role = role::Guard::new(ctx);
                    blocked = role::with_held(ctx, |s| s.channels.endpoint_of(inner[1]).0);
                }
                for id in 1..=2 {
                    let mut value = [1u64, group * 2 + id]; let mut status = 99;
                    hirsel_rt_v2_channel_send(ctx, &mut status, &mut inner[0], value.as_mut_ptr().cast());
                    assert_eq!(status, crate::channels::DONE);
                }
                let mut value = [1u64, inner[1]]; let mut status = 99;
                hirsel_rt_v2_channel_send(ctx, &mut status, &mut outer[0], value.as_mut_ptr().cast());
                assert_eq!(status, crate::channels::DONE);
                let _role = role::Guard::new(ctx);
                assert_eq!(close_endpoint(ctx, inner[0], 0, 0), STATUS_OK);
            }
            { let _role = role::Guard::new(ctx); assert_eq!(close_endpoint(ctx, outer[0], 0, 0), STATUS_OK); }
            let shared = Arc::new(Shared { receiver: outer[1], blocked, task: AtomicU32::new(u32::MAX), polls: AtomicU32::new(0), ordinal: AtomicU64::new(0), saved: AtomicBool::new(false), go: AtomicBool::new(false) });
            let hook = shared.clone();
            crate::pool::free::test_after_compact(ctx, move |task, _, pooled| { if task == hook.task.load(Ordering::Acquire) { assert!(pooled); hook.saved.store(true, Ordering::Release); } });
            let mut scope = 0; hirsel_rt_v2_task_scope_enter(ctx, ptr::null_mut(), &mut scope, &0);
            let env = crate::alloc::hirsel_rt_v2_alloc(ctx, 5, 8, 8, crate::alloc::DOMAIN_HEAP, 0);
            crate::alloc::payload(env).cast::<*const Shared>().write(Arc::as_ptr(&shared));
            let mut task = 0; hirsel_rt_v2_task_spawn(ctx, 0, 0, ptr::null_mut(), &mut task, &scope, child, env, 0);
            wait_until("nested receiver physically saves", || {
                if shared.saved.load(Ordering::Acquire) { return true; }
                let _role = role::Guard::new(ctx);
                crate::tasks::free::suspend(&mut *ctx, true);
                false
            });
            if stopping {
                // The saved child has acknowledged physical stack release.
                // Drop this transient owner before the terminal nonlocal stop.
                drop(shared);
                stopper.stop();
                crate::tasks::park_final(ctx);
            }
            let token = { let _role = role::Guard::new(ctx); role::with_held(ctx, |s| {
                let task = shared.task.load(Ordering::Acquire);
                assert!(s.task(task).stack.is_none());
                crate::pool::free::token((*ctx).run_ptr(), s.key(task), Owner::Any).unwrap()
            }) };
            crate::pool::free::notify((*ctx).run_ptr(), &token); crate::pool::free::notify((*ctx).run_ptr(), &token);
            wait_until("duplicate wakes recheck original inner receiver", || {
                if shared.polls.load(Ordering::Acquire) >= 2 { return true; }
                let _role = role::Guard::new(ctx);
                crate::tasks::free::suspend(&mut *ctx, true);
                false
            });
            shared.go.store(true, Ordering::Release); crate::pool::free::notify((*ctx).run_ptr(), &token); drop(token);
            let mut answer = none; hirsel_rt_v2_task_join(ctx, 0, 0, &scope, &mut task, answer.as_mut_ptr().cast(), 16);
            assert_eq!(answer, [1, 1234]); hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &scope, 0);
            STATUS_OK
        });
        if stopping {
            assert_eq!(outcome.ended, Err(crate::stop::Stop::Requested));
            wait_until("stopped nested receiver storage reclaimed", || control.status().reclamation_pending == 0);
            assert_eq!(ORDER.load(Ordering::Acquire), 0, "stop abandons language cleanup");
        } else {
            assert_eq!(outcome.ended, Ok(STATUS_OK)); assert_eq!(outcome.pending, 0);
        }
        recording.recorder.stop(if stopping {"stopped"} else {"returned"}, None).unwrap();
    } }
}
