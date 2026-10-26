//! Controlled parent/child save gates for the owned exit join.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

#[derive(Default)]
struct Shared {
    parent: AtomicU32,
    child: AtomicU32,
    body_go: AtomicBool,
    rest_go: AtomicBool,
    resting: AtomicBool,
    done: AtomicBool,
    saves: AtomicU32,
    leaves: AtomicU32,
    release_save: AtomicU32,
    calls: AtomicU32,
    exiting: AtomicBool,
    coordinated: AtomicBool,
    deadline_mode: u32,
    due: AtomicBool,
    timer: AtomicU32,
    arms: AtomicU32,
    finishes: AtomicU32,
    replay: bool,
}

fn wait(what: &str, condition: impl Fn() -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !condition() {
        assert!(std::time::Instant::now() < until, "{what}");
        std::thread::yield_now();
    }
}

unsafe fn shared<'a>(env: *mut Header) -> &'a Shared {
    unsafe { &**crate::alloc::payload(env).cast::<*const Shared>() }
}

unsafe fn env(ctx: *mut TaskCtx, shared: &Shared) -> *mut Header {
    let env =
        unsafe { crate::alloc::hirsel_rt_v2_alloc(ctx, 2, 8, 8, crate::alloc::DOMAIN_HEAP, 0) };
    unsafe {
        crate::alloc::payload(env)
            .cast::<*const Shared>()
            .write(shared)
    };
    env
}

unsafe fn gate(ctx: *mut TaskCtx, ready: &AtomicBool) {
    while !ready.load(Ordering::Acquire) {
        unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0).unwrap() };
    }
}

unsafe extern "C" fn child(ctx: *mut TaskCtx, environment: *mut Header, _: u64, _: u64) -> u8 {
    let shared = unsafe { shared(environment) };
    shared
        .child
        .store(unsafe { (*ctx).task_id }, Ordering::Release);
    unsafe { gate(ctx, &shared.body_go) };
    let mut canceled = 99;
    unsafe { hirsel_rt_v2_task_canceled(ctx, &mut canceled, &0) };
    let expected = match shared.deadline_mode {
        2 => CANCEL_DEADLINE + 1,
        4 => CANCEL_PARENT + 1,
        _ => 0,
    };
    assert_eq!(
        canceled, expected,
        "normal joins do not invent cancellation"
    );
    let mut value = [1u64, 7];
    unsafe {
        hirsel_rt_v2_task_finish(
            ctx,
            value.as_mut_ptr().cast(),
            [0u64; 2].as_ptr().cast(),
            16,
            u32::MAX,
        )
    };
    assert_eq!(value, [1, 7], "discard leaves Some in its child's owner");
    shared.resting.store(true, Ordering::Release);
    unsafe { gate(ctx, &shared.rest_go) };
    shared.done.store(true, Ordering::Release);
    STATUS_OK
}

unsafe extern "C" fn dispatch(
    ctx: *mut TaskCtx,
    environment: *mut Header,
    _: u64,
    _: u64,
    frame: *mut u8,
) -> u8 {
    unsafe {
        let shared = shared(environment);
        let scope = frame.add(8).cast::<u64>();
        if frame.cast::<u32>().read() == 0 {
            shared.parent.store((*ctx).task_id, Ordering::Release);
            shared.calls.fetch_add(1, Ordering::Relaxed);
            frame.cast::<u32>().write(2);
            crate::continuation::hirsel_rt_v2_resume_point(ctx, frame, 2);
        }
        if frame.cast::<u32>().read() == 2 {
            let kind = if shared.deadline_mode == 0 { 16 } else { 17 };
            crate::continuation::binding::hirsel_rt_v2_resume_tail(ctx, frame, 2, kind, 40, 71, 72);
            let entered = if shared.deadline_mode == 0 {
                hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), scope, &0)
            } else {
                hirsel_rt_v2_task_scope_enter_until(
                    ctx,
                    std::ptr::null_mut(),
                    scope,
                    &0,
                    &[11, if shared.deadline_mode == 4 { 0 } else { 100 }],
                )
            };
            if entered == crate::STATUS_SUSPENDED {
                return entered;
            }
            assert_eq!(entered, STATUS_OK);
            let mut child_word = 0;
            hirsel_rt_v2_task_spawn(
                ctx,
                0,
                0,
                std::ptr::null_mut(),
                &mut child_word,
                scope,
                child,
                env(ctx, shared),
                0,
            );
            shared.child.store(child_word as u32, Ordering::Release);
            // Stand in for the compiled owned callback result. Its only owner
            // remains in the retained frame until the original exit completes.
            frame
                .add(16)
                .cast::<*mut Vec<u64>>()
                .write(Box::into_raw(Box::new(vec![41, 42])));
            frame.cast::<u32>().write(1);
            assert_eq!(
                crate::continuation::hirsel_rt_v2_resume_point(ctx, frame, 1),
                0
            );
        }
        shared.exiting.store(true, Ordering::Release);
        let until = shared.deadline_mode != 0;
        crate::continuation::binding::hirsel_rt_v2_resume_tail(
            ctx,
            frame,
            1,
            if until { 19 } else { 18 },
            40,
            73,
            74,
        );
        let status = if until {
            hirsel_rt_v2_task_scope_exit_until(ctx, 73, 74, frame.add(24), scope, 0)
        } else {
            hirsel_rt_v2_task_scope_exit(ctx, 73, 74, scope, 0)
        };
        if status == crate::STATUS_SUSPENDED {
            return status;
        }
        assert_eq!(status, STATUS_OK);
        if until {
            assert_eq!(frame.add(24).read(), u8::from(shared.deadline_mode >= 2));
        }
        assert!(shared.done.load(Ordering::Acquire));
        let result = Box::from_raw(frame.add(16).cast::<*mut Vec<u64>>().read());
        assert_eq!(*result, [41, 42]);
        drop(result);
        STATUS_OK
    }
}

unsafe extern "C" fn parent(ctx: *mut TaskCtx, env: *mut Header, caller: u64, site: u64) -> u8 {
    unsafe { crate::continuation::hirsel_rt_v2_resume_run(ctx, env, caller, site, dispatch, 40) }
}

unsafe fn tables<R>(run: usize, action: impl FnOnce(&mut Scheduler) -> R) -> R {
    let run = run as *mut crate::context::RunShared;
    let _guard = unsafe { role::IdleGuard::new(run) };
    let scheduler = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
    action(scheduler)
}

unsafe fn release(run: usize, shared: &Shared, rest: bool) {
    let ready = if rest {
        &shared.rest_go
    } else {
        &shared.body_go
    };
    ready.store(true, Ordering::Release);
    unsafe {
        tables(run, |s| {
            let child = shared.child.load(Ordering::Acquire);
            s.task(child).retried = true;
            s.wake(child);
            super::super::super::publish_ready(s, run as *mut crate::context::RunShared);
        })
    };
}

#[test]
fn compact_exit_retains_owned_result_and_reverse_rest_until_both_images_are_saved() {
    for width in [1, 4] {
        for early in [false, true] {
            run_case(width, early, 0, None, None);
        }
    }
}

fn run_case(
    width: u32,
    early_rest: bool,
    deadline_mode: u32,
    recording: Option<Arc<super::super::super::record::Recording>>,
    replay: Option<Arc<super::super::super::replay::Replay>>,
) {
    let shared = Arc::new(Shared {
        deadline_mode,
        replay: replay.is_some(),
        ..Shared::default()
    });
    shared.parent.store(u32::MAX, Ordering::Relaxed);
    let observe = shared.clone();
    let coordinator = Arc::new(std::sync::Mutex::new(None));
    let spawned = coordinator.clone();
    let mut run = crate::host::BodyRun::new(width, false);
    run.free = true;
    if deadline_mode != 0 {
        run.adapter = Some(Box::new(Clock {
            home: std::thread::current().id(),
            shared: shared.clone(),
        }));
    }
    let install = recording.clone();
    let replayed = replay.clone();
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
    run.config.layouts = layouts.as_ptr();
    run.config.layout_count = 1;
    unsafe { run.use_physical_dispatch() };
    let outcome = run.run(
        move |ctx| unsafe {
            if let Some(replay) = &replayed {
                replay.root(ctx);
            } else {
                super::super::super::enable(ctx);
            }
            if let Some(recording) = &install {
                recording.root(ctx);
            }
        },
        move |ctx| unsafe {
            let run = (*ctx).run_ptr() as usize;
            let hook = shared.clone();
            crate::pool::free::test_before_save(ctx, move |task, complete| {
                if !complete
                    && hook.exiting.load(Ordering::Acquire)
                    && task == hook.parent.load(Ordering::Acquire)
                {
                    let stage = tables(run, |s| {
                        match s.task(task).scope_exits.last().unwrap().phase {
                            Phase::Settle => 1,
                            Phase::RestWaiting(_) => 2,
                            _ => 0,
                        }
                    });
                    if stage != 0
                        && hook
                            .saves
                            .compare_exchange(stage - 1, stage, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                    {
                        wait("parent save gate", || {
                            hook.release_save.load(Ordering::Acquire) >= stage
                        });
                    }
                }
            });
            let hook = shared.clone();
            crate::pool::free::test_after_compact(ctx, move |task, _, pooled| {
                if task == hook.parent.load(Ordering::Acquire) {
                    assert!(pooled);
                    hook.leaves.fetch_add(1, Ordering::AcqRel);
                }
            });
            let mut outer = 0;
            hirsel_rt_v2_task_scope_enter(ctx, std::ptr::null_mut(), &mut outer, &0);
            let mut parent_word = 0;
            hirsel_rt_v2_task_spawn(
                ctx,
                0,
                0,
                std::ptr::null_mut(),
                &mut parent_word,
                &outer,
                parent,
                env(ctx, &shared),
                0,
            );
            let run = (*ctx).run_ptr() as usize;
            let coordinated = shared.clone();
            let shared = shared.clone();
            *spawned.lock().unwrap() = Some(std::thread::spawn(move || {
                wait("parent before first save", || {
                    shared.saves.load(Ordering::Acquire) >= 1
                });
                let token = tables(run, |s| {
                    let me = shared.parent.load(Ordering::Acquire);
                    assert!(!readiness::saved(s, me));
                    assert!(s.task(me).stack.is_some());
                    assert!(matches!(
                        s.task(me).scope_exits.last().unwrap().phase,
                        Phase::Settle
                    ));
                    crate::pool::free::token(
                        run as *mut crate::context::RunShared,
                        s.key(me),
                        Owner::Any,
                    )
                    .unwrap()
                });
                crate::pool::free::notify(run as *mut crate::context::RunShared, &token);
                crate::pool::free::notify(run as *mut crate::context::RunShared, &token);
                shared.release_save.store(1, Ordering::Release);
                wait("parent saved at X2", || {
                    tables(run, |s| {
                        let me = shared.parent.load(Ordering::Acquire);
                        readiness::saved(s, me) && s.task(me).stack.is_none()
                    })
                });
                assert!(!shared.done.load(Ordering::Acquire));
                if deadline_mode == 2 {
                    expire(run, &shared);
                }
                release(run, &shared, false);
                wait("parent before rest save", || {
                    shared.saves.load(Ordering::Acquire) >= 2
                });
                tables(run, |s| {
                    let me = shared.parent.load(Ordering::Acquire);
                    let child = shared.child.load(Ordering::Acquire);
                    assert!(!readiness::saved(s, me));
                    let child = s.key(child);
                    assert_eq!(
                        s.task(me).scope_exits.last().unwrap().phase,
                        Phase::RestWaiting(child)
                    );
                });
                tables(run, |s| {
                    let me = shared.parent.load(Ordering::Acquire);
                    let scope = s.task(me).scope_exits.last().unwrap().scope.slot;
                    if deadline_mode != 0 {
                        let d = s.scope(scope).deadline.as_ref().unwrap();
                        assert_eq!(
                            d.token.is_some(),
                            !shared.replay && deadline_mode < 2
                                || !shared.replay && deadline_mode == 3
                        );
                    }
                });
                if early_rest && width == 4 {
                    release(run, &shared, true);
                    wait("child done before parent save", || {
                        shared.done.load(Ordering::Acquire)
                    });
                }
                shared.release_save.store(2, Ordering::Release);
                if !(early_rest && width == 4) {
                    wait("parent leaves during child rest", || {
                        tables(run, |s| {
                            let me = shared.parent.load(Ordering::Acquire);
                            readiness::saved(s, me) && s.task(me).stack.is_none()
                        })
                    });
                    wait("child rest entered", || {
                        shared.resting.load(Ordering::Acquire)
                    });
                    if deadline_mode == 3 {
                        expire(run, &shared);
                    }
                    crate::pool::free::notify(run as *mut crate::context::RunShared, &token);
                    crate::pool::free::notify(run as *mut crate::context::RunShared, &token);
                    release(run, &shared, true);
                }
                drop(token);
                shared.coordinated.store(true, Ordering::Release);
            }));
            while !coordinated.coordinated.load(Ordering::Acquire) {
                super::super::super::try_yield(&mut *ctx).unwrap();
            }
            hirsel_rt_v2_task_scope_exit(ctx, 0, 0, &outer, 0);
            STATUS_OK
        },
    );
    coordinator.lock().unwrap().take().unwrap().join().unwrap();
    if replay.is_some() {
        assert!(
            matches!(outcome.ended, Err(crate::stop::Stop::Blocked(_))),
            "{outcome:?}"
        );
    } else {
        assert_eq!(outcome.ended, Ok(STATUS_OK));
    }
    assert_eq!(outcome.pending, 0);
    assert_eq!(observe.calls.load(Ordering::Acquire), 1);
    assert!(observe.leaves.load(Ordering::Acquire) >= 2);
    assert!(observe.done.load(Ordering::Acquire));
    if deadline_mode != 0 {
        assert_eq!(
            observe.arms.load(Ordering::Acquire),
            if replay.is_some() {
                0
            } else if matches!(deadline_mode, 2 | 3) {
                2
            } else {
                1
            }
        );
        assert_eq!(
            observe.finishes.load(Ordering::Acquire),
            u32::from(replay.is_none())
        );
    }
}

unsafe fn expire(run: usize, shared: &Shared) {
    if shared.replay {
        return;
    }
    shared.due.store(true, Ordering::Release);
    unsafe { crate::pool::wake_home(run as *mut crate::context::RunShared) };
    wait("deadline expires while owned exit waits", || unsafe {
        tables(run, |s| {
            let me = shared.parent.load(Ordering::Acquire);
            let scope = s.task(me).scope_exits.last().unwrap().scope.slot;
            s.scope(scope).deadline.as_ref().unwrap().expired
        })
    });
}

struct Clock {
    home: std::thread::ThreadId,
    shared: Arc<Shared>,
}
impl crate::io::Adapter for Clock {
    fn preflight(
        &self,
        _: &crate::io::Request,
    ) -> Result<Option<crate::io::Completion>, crate::io::AdapterError> {
        Ok(None)
    }
    fn enter(&mut self, token: u64) {
        self.shared.timer.store(token as u32, Ordering::Release);
    }
    fn call(
        &mut self,
        request: crate::io::Request,
    ) -> Result<crate::io::Completion, crate::io::AdapterError> {
        assert_eq!(self.home, std::thread::current().id());
        assert!(!self.shared.replay, "replay never calls a live scope timer");
        assert!(matches!(
            request,
            crate::io::Request::SleepUntil { clock: 11, .. }
        ));
        self.shared.arms.fetch_add(1, Ordering::Relaxed);
        if self.shared.deadline_mode == 4 || self.shared.due.load(Ordering::Acquire) {
            Ok(crate::io::Completion::Wait(Ok(())))
        } else {
            Err(crate::io::AdapterError::Blocked)
        }
    }
    fn ready(&mut self) -> Result<crate::io::Ready, crate::io::AdapterError> {
        assert_eq!(self.home, std::thread::current().id());
        assert!(!self.shared.replay);
        Ok(crate::io::Ready::Tokens(
            if self.shared.due.load(Ordering::Acquire) {
                vec![u64::from(self.shared.timer.load(Ordering::Acquire))]
            } else {
                vec![]
            },
        ))
    }
    fn finish(&mut self, _: u64) {
        assert_eq!(self.home, std::thread::current().id());
        assert!(!self.shared.replay);
        self.shared.finishes.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn compact_exit_timer_survives_body_and_rest_waits_and_replays_without_live_timers() {
    for mode in 0..=4 {
        let recording = super::super::super::record::Recording::new(b"compact scope timer", false);
        for width in [1] {
            run_case(width, false, mode, Some(recording.clone()), None);
        }
        recording.recorder.stop("returned", None).unwrap().unwrap();
        let expected = recording.trace();
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        let parsed = hirsel_free::tape::parse(&bytes).unwrap();
        let expected = hirsel_free::projection::Projection::new("returned", &parsed.cut, &expected);
        for width in [1, 4] {
            let replay = super::super::super::replay::Replay::new(&bytes).unwrap();
            run_case(width, false, mode, None, Some(replay.clone()));
            assert_eq!(replay.finish().unwrap(), expected);
        }
    }
}
