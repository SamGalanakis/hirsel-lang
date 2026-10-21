//! Deadline calls stay outside recorder sections and on their home adapter.
use super::*;
use crate::io::{Adapter, AdapterError, Completion, Request};
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use hirsel_free::replay::Replayer;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

struct Clock {
    home: std::thread::ThreadId,
    recording: Arc<super::super::record::Recording>,
    calls: Arc<AtomicU64>,
    finishes: Arc<AtomicU64>,
    exit_op: u64,
}
impl Adapter for Clock {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(std::thread::current().id(), self.home);
        // A synchronous cut here also proves the timer call holds no section.
        let cut = self.recording.recorder.flush().unwrap().unwrap();
        assert_eq!(
            cut.progress[&Vec::new()],
            Progress::new(1, Phase::Entered),
            "timer issue follows its recorded scope entry"
        );
        self.calls.fetch_add(1, Ordering::Relaxed);
        let Request::SleepUntil {
            clock: 11,
            deadline,
        } = request
        else {
            panic!("unexpected {request:?}")
        };
        if deadline == 0 {
            Ok(Completion::Wait(Ok(())))
        } else {
            Err(AdapterError::Blocked)
        }
    }
    fn finish(&mut self, _: u64) {
        assert_eq!(std::thread::current().id(), self.home);
        let cut = self.recording.recorder.flush().unwrap().unwrap();
        let progress = cut.progress[&Vec::new()];
        assert!(
            progress == Progress::new(1, Phase::Entered)
                || progress == Progress::new(self.exit_op, Phase::Entered),
            "timer cleanup is outside the entry/exit section: {progress:?}"
        );
        self.finishes.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn native_deadline_entry_records_before_home_arm_and_exit_records_expired() {
    for width in [2, 8] {
        for durable in [false, true] {
            for expired in [false, true] {
                let recording = super::super::record::Recording::new(b"native deadline", durable);
                let install = recording.clone();
                let calls = Arc::new(AtomicU64::new(0));
                let finishes = Arc::new(AtomicU64::new(0));
                let mut run = crate::host::BodyRun::new(width, true);
                run.free = true;
                run.adapter = Some(Box::new(Clock {
                    home: std::thread::current().id(),
                    recording: recording.clone(),
                    calls: calls.clone(),
                    finishes: finishes.clone(),
                    exit_op: 4,
                }));
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        let mut scope = 0;
                        let at = [11, if expired { 0 } else { 100 }];
                        assert_eq!(
                            hirsel_rt_v2_task_scope_enter_until(
                                ctx,
                                std::ptr::null_mut(),
                                &mut scope,
                                &0,
                                &at
                            ),
                            STATUS_OK
                        );
                        let mut canceled = 99;
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                        assert_eq!(canceled, if expired { 1 + CANCEL_DEADLINE } else { 0 });
                        crate::finalize::hirsel_rt_v2_finalize_begin(ctx, 0);
                        hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0);
                        assert_eq!(canceled, 0, "finalizers shield deadline cancellation");
                        crate::finalize::hirsel_rt_v2_finalize_end(ctx, 0);
                        let mut observed = 99;
                        assert_eq!(
                            hirsel_rt_v2_task_scope_exit_until(ctx, 0, 0, &mut observed, &scope, 0),
                            STATUS_OK
                        );
                        assert_eq!(observed, u8::from(expired));
                        assert_eq!((*ctx).run_ref().host_waits.load(Ordering::Acquire), 0);
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(calls.load(Ordering::Relaxed), 1);
                assert_eq!(finishes.load(Ordering::Relaxed), 1);
                let cut = recording
                    .recorder
                    .stop("returned", None)
                    .unwrap()
                    .unwrap()
                    .0;
                assert_eq!(cut.progress[&Vec::new()], Progress::new(4, Phase::Done));
                let trace = recording.trace();
                let marked = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink();
                let tape = hirsel_free::tape::parse(&marked.bytes).unwrap();
                let mut replay = Replayer::new(&tape).unwrap();
                for op in 1..=4 {
                    if op == 2 {
                        assert_eq!(
                            replay.cancel_check(&[], op).unwrap(),
                            expired.then_some(CANCEL_DEADLINE)
                        );
                    }
                    if op == 4 {
                        assert_eq!(replay.expired(&[], op).unwrap(), expired);
                    }
                    replay.publish(&[], Progress::new(op, Phase::Done)).unwrap();
                }
                assert_eq!(
                    replay.finish(&cut.progress).unwrap(),
                    Projection::new("returned", &cut.progress, &trace)
                );
            }
        }
    }
}

unsafe extern "C" fn held_result(ctx: *mut TaskCtx, env: *mut Header, _: u64, _: u64) -> u8 {
    let scope = unsafe { crate::alloc::payload(env).cast::<u64>().read() };
    let mut image = [1u64, 7];
    let none = [0u64; 2];
    unsafe { hirsel_rt_v2_task_finish(ctx, image.as_mut_ptr().cast(), none.as_ptr().cast(), 16) };
    assert_eq!(image, [1, 7], "scope exit discards in the child's frame");
    for _ in 0..130 {
        let mut canceled = 99;
        unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) };
        assert_eq!(canceled, 0);
        let _role = unsafe { role::Guard::new(ctx) };
        let deadline = scheduler(ctx)
            .scope(scope as u32)
            .deadline
            .as_ref()
            .unwrap();
        assert!(
            deadline.token.is_some(),
            "the scope retains its armed timer through cleanup"
        );
        assert!(!deadline.expired);
    }
    STATUS_OK
}

#[test]
fn native_scope_exit_keeps_its_timer_until_the_saved_child_rest_returns() {
    for width in [2, 8] {
        for durable in [false, true] {
            let recording = super::super::record::Recording::new(b"native deadline rest", durable);
            let install = recording.clone();
            let calls = Arc::new(AtomicU64::new(0));
            let finishes = Arc::new(AtomicU64::new(0));
            let mut run = crate::host::BodyRun::new(width, true);
            run.free = true;
            run.adapter = Some(Box::new(Clock {
                home: std::thread::current().id(),
                recording: recording.clone(),
                calls: calls.clone(),
                finishes: finishes.clone(),
                exit_op: 3,
            }));
            let layout = crate::Layout {
                magic: crate::LAYOUT_MAGIC,
                version: crate::LAYOUT_VERSION,
                size: 8,
                align: 8,
                finalizer: None,
                steps: std::ptr::null(),
                step_count: 0,
                field_count: 1,
                flags: 0,
                logical: 0,
                logical_count: std::ptr::null(),
                scan: std::ptr::null(),
                scan_words: 0,
            };
            let layouts = [&raw const layout];
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
                    hirsel_rt_v2_task_scope_enter_until(
                        ctx,
                        std::ptr::null_mut(),
                        &mut scope,
                        &0,
                        &[11, 100],
                    );
                    let env = crate::alloc::hirsel_rt_v2_alloc(
                        ctx,
                        2,
                        8,
                        8,
                        crate::alloc::DOMAIN_HEAP,
                        0,
                    );
                    crate::alloc::payload(env).cast::<u64>().write(scope);
                    let mut child = 0;
                    hirsel_rt_v2_task_spawn(
                        ctx,
                        0,
                        0,
                        std::ptr::null_mut(),
                        &mut child,
                        &scope,
                        held_result,
                        env,
                        0,
                    );
                    let mut expired = 99;
                    assert_eq!(
                        hirsel_rt_v2_task_scope_exit_until(ctx, 0, 0, &mut expired, &scope, 0),
                        STATUS_OK
                    );
                    assert_eq!(expired, 0);
                    assert_eq!((*ctx).run_ref().host_waits.load(Ordering::Acquire), 0);
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            assert_eq!(calls.load(Ordering::Relaxed), 1);
            assert_eq!(finishes.load(Ordering::Relaxed), 1);
            let cut = recording
                .recorder
                .stop("returned", None)
                .unwrap()
                .unwrap()
                .0;
            assert_eq!(cut.progress[&Vec::new()], Progress::new(3, Phase::Done));
            assert_eq!(cut.progress[&vec![0]], Progress::new(132, Phase::Done));
        }
    }
}
