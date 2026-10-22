use super::*;
use crate::io::WaitError;
use hirsel_run_schema::ProcessFailure;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

#[derive(Default)]
struct Probe {
    run: AtomicPtr<crate::context::RunShared>,
    preflight: AtomicU32,
    effects: AtomicU32,
    closes: AtomicU32,
}
struct Home {
    thread: std::thread::ThreadId,
    probe: Arc<Probe>,
    mode: u8,
}
fn request(mode: u8) -> Request {
    match mode {
        3 | 4 => Request::ChildWait {
            process: 17,
            child: 18,
            deadline: None,
        },
        5 => Request::Commit {
            directory: 17,
            file: 18,
        },
        6 => Request::Stdout { io: 1 },
        7 => Request::Print {
            stdout: 17,
            bytes: vec![42],
        },
        _ => Request::SleepUntil {
            clock: 17,
            deadline: 100,
        },
    }
}
fn expected(mode: u8) -> Result<Completion, AdapterError> {
    Ok(match mode {
        1 | 8 => Completion::Wait(Ok(())),
        2 => return Err(AdapterError::Unsupported("preflight failure".into())),
        3 => Completion::ChildExit(Err(ProcessFailure::Canceled(3))),
        4 => Completion::ChildExit(Err(ProcessFailure::Fault(91))),
        5 | 7 => Completion::Unit(Err(crate::io::IoError::Canceled(3))),
        6 => Completion::Handle(Err(crate::io::IoError::NotGranted)),
        _ => Completion::Wait(Err(WaitError::Canceled(3))),
    })
}
impl Home {
    fn check(&self) {
        assert_eq!(std::thread::current().id(), self.thread);
        let guard = unsafe { role::IdleGuard::try_new(self.probe.run.load(Ordering::Acquire)) };
        assert!(
            guard.is_some(),
            "cancellation validation/cleanup has no role"
        );
    }
}
impl crate::io::Adapter for Home {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        self.check();
        self.probe.preflight.fetch_add(1, Ordering::Release);
        match self.mode {
            2 => Err(AdapterError::Unsupported("preflight failure".into())),
            4 => Ok(Some(Completion::ChildExit(Err(ProcessFailure::Fault(91))))),
            _ => Ok(None),
        }
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        self.check();
        if request == (Request::Close { handle: 18 }) {
            self.probe.closes.fetch_add(1, Ordering::Release);
            return Ok(Completion::Unit(Ok(())));
        }
        self.probe.effects.fetch_add(1, Ordering::Release);
        match self.mode {
            1 | 8 => Ok(Completion::Wait(Ok(()))),
            6 => Ok(Completion::Handle(Err(crate::io::IoError::NotGranted))),
            _ => panic!("canceled request issued its effect"),
        }
    }
    fn cancellable(&self, _: u64) -> bool {
        self.mode != 8
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.thread);
    }
}

struct Direct {
    probe: Arc<Probe>,
    mode: u8,
}
impl Direct {
    fn check(&self) {
        let run = self.probe.run.load(Ordering::Acquire);
        assert!(!run.is_null());
        drop(unsafe { role::IdleGuard::new(run) });
    }
}
impl crate::io_concurrent::ConcurrentAdapter for Direct {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        self.check();
        self.probe.preflight.fetch_add(1, Ordering::Release);
        match self.mode {
            2 => Err(AdapterError::Unsupported("preflight failure".into())),
            4 => Ok(Some(Completion::ChildExit(Err(ProcessFailure::Fault(91))))),
            _ => Ok(None),
        }
    }
    fn call(&self, request: Request) -> Result<Completion, AdapterError> {
        self.check();
        if request == (Request::Close { handle: 18 }) {
            self.probe.closes.fetch_add(1, Ordering::Release);
            return Ok(Completion::Unit(Ok(())));
        }
        self.probe.effects.fetch_add(1, Ordering::Release);
        match self.mode {
            1 | 8 => Ok(Completion::Wait(Ok(()))),
            6 => Ok(Completion::Handle(Err(crate::io::IoError::NotGranted))),
            _ => panic!("canceled concurrent request issued its effect"),
        }
    }
    fn cancellable(&self, _: u64) -> bool {
        self.mode != 8
    }
}

#[test]
fn native_home_cancellation_entry_validates_before_effect_and_preserves_consumed_cleanup() {
    cancellation_entries(false);
}

#[test]
fn native_concurrent_cancellation_validates_before_effect_and_preserves_consumed_cleanup() {
    cancellation_entries(true);
}

fn cancellation_entries(concurrent: bool) {
    for width in [1, 8] {
        for durable in [false, true] {
            for mode in 0..9 {
                let recording =
                    super::super::record::Recording::new(b"native home cancellation", durable);
                let install = recording.clone();
                let probe = Arc::new(Probe::default());
                let observed = probe.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                if !concurrent {
                    run.adapter = Some(Box::new(Home {
                        thread: std::thread::current().id(),
                        probe: probe.clone(),
                        mode,
                    }));
                }
                let direct = Direct {
                    probe: probe.clone(),
                    mode,
                };
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        let _role = role::Guard::new(ctx);
                        observed.run.store((*ctx).run_ptr(), Ordering::Release);
                        scheduler(ctx).cancel(0, 3);
                        if mode == 1 {
                            (*ctx).shield = 1;
                        }
                        let result = if concurrent {
                            crate::stop::check(ctx, prepare(ctx, request(mode)));
                            assert!(crate::stop::check(ctx, claim(ctx)));
                            crate::stop::check(ctx, invoke_concurrent(ctx, &direct));
                            crate::stop::check(ctx, ended(ctx));
                            crate::stop::check(ctx, complete_result(ctx))
                        } else {
                            crate::stop::check(
                                ctx,
                                request_home(ctx, request(mode), KeySet::coupled()),
                            )
                        };
                        assert_eq!(result, expected(mode));
                        (*ctx).shield = 0;
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let issues = matches!(mode, 1 | 6 | 8);
                assert_eq!(probe.preflight.load(Ordering::Acquire), u32::from(!issues));
                assert_eq!(probe.effects.load(Ordering::Acquire), u32::from(issues));
                assert_eq!(
                    probe.closes.load(Ordering::Acquire),
                    u32::from(matches!(mode, 3..=5))
                );
                let cut = recording
                    .recorder
                    .stop("returned", None)
                    .unwrap()
                    .unwrap()
                    .0;
                let expected_projection = hirsel_free::projection::Projection::new(
                    "returned",
                    &cut.progress,
                    &recording.trace(),
                );
                let bytes = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink()
                    .bytes;
                let parsed = hirsel_free::tape::parse(&bytes).unwrap();
                let records = hirsel_free::record::decode(&parsed.logs[&Vec::new()]).unwrap();
                assert_eq!(
                    records
                        .iter()
                        .filter(|r| matches!(
                            r,
                            hirsel_free::record::Record::Cancel { cause: Some(3), .. }
                        ))
                        .count(),
                    usize::from(!matches!(mode, 1 | 6))
                );
                for width in [1, 8] {
                    let replay = super::super::replay::Replay::new(&bytes).unwrap();
                    let install = replay.clone();
                    let mut run = crate::host::BodyRun::new(width, false);
                    run.free = true;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        move |ctx| unsafe {
                            install.root(ctx);
                        },
                        move |ctx| unsafe {
                            let _role = role::Guard::new(ctx);
                            assert_eq!(
                                scheduler(ctx).canceled(0),
                                None,
                                "replay has no live cancellation"
                            );
                            if mode == 1 {
                                (*ctx).shield = 1;
                            }
                            let result = crate::stop::check(
                                ctx,
                                replay_request(ctx, request(mode), KeySet::coupled()),
                            );
                            assert_eq!(result, expected(mode));
                            (*ctx).shield = 0;
                            super::super::replay::start(ctx);
                            unreachable!("cancellation root past cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(replay.finish().unwrap(), expected_projection);
                }
            }
        }
    }
}

#[derive(Default)]
struct ParkProbe {
    run: AtomicPtr<crate::context::RunShared>,
    parked: std::sync::atomic::AtomicBool,
    canceled: std::sync::atomic::AtomicBool,
    calls: AtomicU32,
    live: bool,
}
struct ParkHome {
    probe: Arc<ParkProbe>,
    thread: std::thread::ThreadId,
    mode: u8,
    token: Option<u64>,
}
impl crate::io::Adapter for ParkHome {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn enter(&mut self, token: u64) {
        self.token = Some(token);
    }
    fn call(&mut self, _: Request) -> Result<Completion, AdapterError> {
        assert_eq!(std::thread::current().id(), self.thread);
        self.probe.calls.fetch_add(1, Ordering::Release);
        if self.probe.canceled.load(Ordering::Acquire) {
            assert_ne!(
                self.mode, 0,
                "canceled uncommitted request reissued its effect"
            );
            Ok(Completion::Wait(Ok(())))
        } else {
            Err(AdapterError::Blocked)
        }
    }
    fn parked(&mut self, _: u64) {
        self.probe.parked.store(true, Ordering::Release);
        let run = self.probe.run.load(Ordering::Acquire);
        let _guard = unsafe { role::IdleGuard::new(run) };
        let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
        for id in std::mem::take(&mut s.retrying) {
            s.task(id).retried = true;
            s.wake(id);
        }
        unsafe {
            crate::tasks::readiness::poll_free(s);
            super::super::publish_ready(s, run);
        }
    }
    fn ready(&mut self) -> Result<crate::io::Ready, AdapterError> {
        assert_eq!(std::thread::current().id(), self.thread);
        Ok(crate::io::Ready::Tokens(
            if self.mode != 0 && self.probe.canceled.load(Ordering::Acquire) {
                self.token.into_iter().collect()
            } else {
                vec![]
            },
        ))
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        Ok(false)
    }
    fn cancellable(&self, _: u64) -> bool {
        self.mode != 2
    }
}
unsafe extern "C" fn cancel_parked_owner(ctx: *mut u8, data: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let probe = unsafe { &*data.cast::<ParkProbe>() };
    let role = unsafe { role::Guard::new(ctx) };
    unsafe {
        if probe.live {
            while !probe.parked.load(Ordering::Acquire) {
                let s = scheduler(ctx);
                let me = s.current;
                if !s.retrying.contains(&me) {
                    s.retrying.push(me);
                }
                crate::stop::check(
                    ctx,
                    try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0),
                );
            }
        }
        super::super::record::begin(ctx);
        crate::stop::check(
            ctx,
            super::super::record::publish(ctx, Phase::Done, |_| {
                if probe.live {
                    scheduler(ctx).cancel(0, 3);
                }
            }),
        );
        probe.canceled.store(true, Ordering::Release);
        if !probe.live {
            super::super::replay::start(ctx);
            unreachable!("cancel child past cut");
        }
        // Exercise the same home notification as ordinary cancellation ops.
        crate::stop::check(ctx, super::super::try_yield(&mut *ctx));
        let s = scheduler(ctx);
        let me = s.current;
        s.task(me).finishing = true;
        drop(role);
        role::enter(ctx);
        crate::pool::free::finish_current(ctx);
    }
}
unsafe fn spawn_canceler(ctx: *mut TaskCtx, probe: &Arc<ParkProbe>) {
    unsafe {
        let stack = Stack::new();
        let child_ctx = stack.context();
        child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
        let path = if probe.live {
            let child = crate::stop::check(ctx, super::super::record::child(ctx));
            let path = child.path().to_vec();
            (*child_ctx).recording = Some(Box::new(child));
            path
        } else {
            let path = vec![0];
            super::super::record::begin(ctx);
            assert!(crate::stop::check(
                ctx,
                super::super::record::publish(ctx, Phase::Done, |_| super::super::replay::child(
                    ctx, &path
                ))
            ));
            path
        };
        let s = scheduler(ctx);
        let id = s.tasks.len() as u32;
        (*child_ctx).task_id = id;
        (*child_ctx).task_sp = stack.prepare(
            cancel_parked_owner,
            child_ctx.cast(),
            Arc::as_ptr(probe).cast_mut().cast(),
        );
        let mut row = crate::tasks::Task::new(1, child_ctx, None, path, None);
        row.stack = Some(stack);
        s.tasks.push(Some(row));
        crate::pool::free::schedule_saved((*ctx).run_ptr(), id, Owner::Any);
    }
}

#[test]
fn saved_native_host_cancellation_wakes_without_readiness_and_respects_shield_and_commit() {
    const CHILD: &str = "HIRSEL_F1_HOST_PARK_CANCEL";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::host::cancellation_tests::saved_native_host_cancellation_wakes_without_readiness_and_respects_shield_and_commit", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(1);
    for width in [1, 8] {
        for durable in [false, true] {
            // Cancellation with no ready token; shielded, then committed,
            // requests instead wait for their genuine source notification.
            for mode in 0..3 {
                let recording =
                    super::super::record::Recording::new(b"parked host cancellation", durable);
                let install = recording.clone();
                let probe = Arc::new(ParkProbe {
                    live: true,
                    ..Default::default()
                });
                let observed = probe.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.adapter = Some(Box::new(ParkHome {
                    probe: probe.clone(),
                    thread: std::thread::current().id(),
                    mode,
                    token: None,
                }));
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        let _role = role::Guard::new(ctx);
                        observed.run.store((*ctx).run_ptr(), Ordering::Release);
                        spawn_canceler(ctx, &observed);
                        if mode == 1 {
                            (*ctx).shield = 1;
                        }
                        let result = crate::stop::check(
                            ctx,
                            request_home(ctx, request(0), KeySet::coupled()),
                        );
                        assert_eq!(result, if mode == 0 { expected(0) } else { expected(1) });
                        (*ctx).shield = 0;
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                assert!(probe.canceled.load(Ordering::Acquire));
                assert_eq!(
                    probe.calls.load(Ordering::Acquire),
                    if mode == 0 { 1 } else { 2 }
                );
                let cut = recording
                    .recorder
                    .stop("returned", None)
                    .unwrap()
                    .unwrap()
                    .0;
                let expected_projection = hirsel_free::projection::Projection::new(
                    "returned",
                    &cut.progress,
                    &recording.trace(),
                );
                let bytes = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink()
                    .bytes;
                for width in [1, 8] {
                    let replay = super::super::replay::Replay::new(&bytes).unwrap();
                    let install = replay.clone();
                    let probe = Arc::new(ParkProbe::default());
                    let mut run = crate::host::BodyRun::new(width, false);
                    run.free = true;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        move |ctx| unsafe {
                            install.root(ctx);
                        },
                        move |ctx| unsafe {
                            let _role = role::Guard::new(ctx);
                            spawn_canceler(ctx, &probe);
                            if mode == 1 {
                                (*ctx).shield = 1;
                            }
                            let result = crate::stop::check(
                                ctx,
                                replay_request(ctx, request(0), KeySet::coupled()),
                            );
                            assert_eq!(result, if mode == 0 { expected(0) } else { expected(1) });
                            (*ctx).shield = 0;
                            super::super::replay::start(ctx);
                            unreachable!("parked cancellation root past cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(replay.finish().unwrap(), expected_projection);
                }
            }
        }
    }
}
