use super::*;
use crate::tasks::free::{record, replay};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

struct Probe {
    run: AtomicPtr<crate::context::RunShared>,
    parked: AtomicBool,
    parked_waiter: std::sync::Mutex<Option<(u32, u32)>>,
    delivered: AtomicBool,
    done: AtomicBool,
    home_dropped: AtomicBool,
    waits: AtomicU32,
    write: std::sync::Mutex<UnixStream>,
    notice: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>,
    replay: bool,
    stop_in_wait: bool,
    expired_timer: bool,
}
struct Home {
    thread: std::thread::ThreadId,
    affine: std::rc::Rc<()>,
    probe: Arc<Probe>,
    read: UnixStream,
    driver: crate::readiness::Readiness,
    token: u64,
    interruptible: bool,
}
impl Home {
    fn check(&self) {
        assert_eq!(std::thread::current().id(), self.thread);
        assert_eq!(std::rc::Rc::strong_count(&self.affine), 1);
        // Other workers may briefly own the role while home polls readiness.
        // Acquiring it here proves this callback holds no role itself; release
        // it before any adapter or driver work, following the home fixture.
        drop(unsafe { role::IdleGuard::new(self.probe.run.load(Ordering::Acquire)) });
    }
}
impl crate::io::Adapter for Home {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        self.check();
        Ok(None)
    }
    fn enter(&mut self, token: u64) {
        self.check();
        self.token = token;
        self.driver.enter(token);
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        self.check();
        assert!(matches!(request, Request::ReadStdin { stdin: 17, max: 1 }));
        if self.probe.delivered.load(Ordering::Acquire) {
            Ok(Completion::Bytes(Ok(b"x".to_vec())))
        } else {
            self.driver.await_descriptor(self.read.as_raw_fd());
            Err(AdapterError::Blocked)
        }
    }
    fn parked(&mut self, token: u64) {
        self.check();
        self.driver.parked(token);
        self.probe.parked.store(true, Ordering::Release);
        if let Some(waiter) = self.probe.parked_waiter.lock().unwrap().take() {
            unsafe {
                crate::tasks::free::home::runner_wakes(
                    self.probe.run.load(Ordering::Acquire),
                    &[waiter],
                );
            }
        }
    }
    fn ready(&mut self) -> Result<crate::io::Ready, AdapterError> {
        self.check();
        self.driver.ready().map(crate::io::Ready::Tokens)
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        self.check();
        assert!(
            self.interruptible,
            "uninterruptible opaque provider is refused before entering its wait"
        );
        self.probe.waits.fetch_add(1, Ordering::Release);
        if let Some(notice) = self.probe.notice.lock().unwrap().take() {
            notice.send(()).unwrap();
        }
        self.driver.wait()
    }
    fn wake_on(&mut self, fd: std::os::fd::RawFd) -> bool {
        self.check();
        self.interruptible && self.driver.wake_on(fd).is_ok()
    }
    fn finish(&mut self, token: u64) {
        self.check();
        self.driver.finish(token);
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.thread);
        self.probe.home_dropped.store(true, Ordering::Release);
    }
}
fn home_request() -> Request {
    Request::ReadStdin { stdin: 17, max: 1 }
}
fn home_keys() -> KeySet {
    KeySet::new(vec![(crate::io_keys::Key(17), Mode::Exclusive)])
}
unsafe fn wait_flag(ctx: *mut TaskCtx, flag: &AtomicBool) {
    while !flag.load(Ordering::Acquire) {
        let s = scheduler(ctx);
        let me = s.current;
        if !s.retrying.contains(&me) {
            s.retrying.push(me);
        }
        unsafe {
            crate::stop::check(
                ctx,
                try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0),
            );
        }
    }
}
unsafe extern "C" fn home_child(ctx: *mut u8, data: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let probe = unsafe { &*data.cast::<Probe>() };
    let role = unsafe { role::Guard::new(ctx) };
    unsafe {
        let result = if probe.replay {
            replay_request(ctx, home_request(), home_keys())
        } else {
            request_home(ctx, home_request(), home_keys())
        };
        assert_eq!(
            crate::stop::check(ctx, result),
            Ok(Completion::Bytes(Ok(b"x".to_vec())))
        );
        probe.done.store(true, Ordering::Release);
        wake_retrying(ctx);
        if probe.replay {
            replay::start(ctx);
            unreachable!("child beyond replay cut");
        }
        let s = scheduler(ctx);
        let me = s.current;
        s.task(me).finishing = true;
        drop(role);
        role::enter(ctx);
        crate::pool::free::finish_current(ctx);
    }
}
unsafe fn spawn_home(ctx: *mut TaskCtx, probe: &Arc<Probe>) {
    unsafe {
        let stack = Stack::new();
        let child_ctx = stack.context();
        child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
        let path = if probe.replay {
            let path = vec![0];
            record::begin(ctx);
            assert!(crate::stop::check(
                ctx,
                record::publish(ctx, Phase::Done, |_| replay::child(ctx, &path))
            ));
            path
        } else {
            let child = crate::stop::check(ctx, record::child(ctx));
            let path = child.path().to_vec();
            (*child_ctx).recording = Some(Box::new(child));
            path
        };
        let s = scheduler(ctx);
        let id = s.tasks.len() as u32;
        (*child_ctx).task_id = id;
        (*child_ctx).task_sp = stack.prepare(
            home_child,
            child_ctx.cast(),
            Arc::as_ptr(probe).cast_mut().cast(),
        );
        let mut row = crate::tasks::Task::new(1, child_ctx, None, path, None);
        row.stack = Some(stack);
        s.tasks.push(Some(row));
        crate::pool::free::schedule_saved(
            (*ctx).run_ptr(),
            crate::tasks::scheduler(ctx).key(id),
            Owner::Any,
        );
    }
}
unsafe fn program(ctx: *mut TaskCtx, probe: &Arc<Probe>, adapter: Option<&dyn ConcurrentAdapter>) {
    unsafe {
        if !probe.replay {
            *probe.parked_waiter.lock().unwrap() =
                Some(crate::tasks::free::home::runner_waiter(ctx));
        }
        spawn_home(ctx, probe);
        if !probe.replay {
            wait_flag(ctx, &probe.parked);
        }
        let mut registry = Registry::default();
        let Completion::ClockGrant(Ok(clock)) = request(
            ctx,
            Request::MonotonicClock { io: 1 },
            adapter,
            &mut registry,
        ) else {
            panic!("timer grant");
        };
        let Completion::Monotonic(Ok(now)) =
            request(ctx, Request::Monotonic { clock }, adapter, &mut registry)
        else {
            panic!("timer sample");
        };
        assert_eq!(
            request(
                ctx,
                Request::SleepUntil {
                    clock,
                    deadline: now + if probe.expired_timer { 0 } else { 20_000_000 }
                },
                adapter,
                &mut registry
            ),
            Completion::Wait(Ok(()))
        );
        // Stop/refusal modes leave the home request pending even when
        // the timer completed before home could enter its opaque wait.
        if !probe.replay && !probe.stop_in_wait {
            assert!(
                probe.parked.load(Ordering::Acquire),
                "custom request saved before timer completed"
            );
            probe.delivered.store(true, Ordering::Release);
            probe.write.lock().unwrap().write_all(b"x").unwrap();
        }
        wait_flag(ctx, &probe.done);
    }
}

#[test]
fn opaque_saved_home_request_progresses_with_concurrent_timer_and_stops_reclaim_both_services() {
    for width in [1, 8] {
        for durable in [false, true] {
            // Success; external stop inside the opaque wait; reject an
            // uninterruptible provider before calling its blocking wait.
            for mode in 0..5 {
                let (read, write) = UnixStream::pair().unwrap();
                let (notice, waiting) = std::sync::mpsc::channel();
                let probe = Arc::new(Probe {
                    run: Default::default(),
                    parked: false.into(),
                    parked_waiter: Default::default(),
                    delivered: false.into(),
                    done: false.into(),
                    home_dropped: false.into(),
                    waits: 0.into(),
                    write: std::sync::Mutex::new(write),
                    notice: std::sync::Mutex::new(matches!(mode, 1 | 3).then_some(notice)),
                    replay: false,
                    stop_in_wait: mode != 0,
                    expired_timer: matches!(mode, 3 | 4),
                });
                let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
                real.discard_history();
                real.clock_grants(true, false);
                let adapter = Arc::new(Timer {
                    real: real
                        .into_concurrent()
                        .unwrap_or_else(|_| panic!("timer transport")),
                    parks: Default::default(),
                });
                let mut admitted = Some(adapter.clone());
                let recording = record::Recording::new(b"opaque saved custom", durable);
                let install = recording.clone();
                let setup = probe.clone();
                let observed = probe.clone();
                let observed_adapter = adapter.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.adapter = Some(Box::new(Home {
                    thread: std::thread::current().id(),
                    affine: std::rc::Rc::new(()),
                    probe: probe.clone(),
                    read,
                    driver: Default::default(),
                    token: 0,
                    interruptible: !matches!(mode, 2 | 4),
                }));
                unsafe {
                    run.use_physical_dispatch();
                }
                let control = run.control();
                let stopper = matches!(mode, 1 | 3).then(|| {
                    std::thread::spawn(move || {
                        waiting
                            .recv_timeout(std::time::Duration::from_secs(5))
                            .unwrap();
                        control.stop();
                    })
                });
                let outcome = run.run(
                    move |ctx| unsafe {
                        crate::tasks::free::enable(ctx);
                        install.root(ctx);
                        setup.run.store((*ctx).run_ptr(), Ordering::Release);
                    },
                    move |ctx| unsafe {
                        let _role = role::Guard::new(ctx);
                        {
                            let _segment = role::Segment::new(ctx);
                            concurrent::Service::install(ctx, admitted.take().unwrap()).unwrap();
                        }
                        program(ctx, &observed, Some(&*observed_adapter));
                        crate::STATUS_OK
                    },
                );
                if let Some(stopper) = stopper {
                    stopper.join().unwrap();
                }
                assert_eq!(outcome.pending, 0);
                assert!(probe.parked.load(Ordering::Acquire));
                match mode {
                    0 => {
                        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                        assert!(probe.done.load(Ordering::Acquire));
                    }
                    1 | 3 => assert_eq!(outcome.ended, Err(Stop::Requested)),
                    _ => {
                        assert_eq!(
                            outcome.ended,
                            Err(Stop::Refused(
                                "opaque mixed wait needs interruptible home wait".into()
                            ))
                        );
                        assert_eq!(probe.waits.load(Ordering::Acquire), 0);
                    }
                }
                assert!(probe.home_dropped.load(Ordering::Acquire));
                assert_eq!(
                    Arc::strong_count(&adapter),
                    1,
                    "no service alias survived teardown"
                );
                assert_eq!(
                    Arc::strong_count(&probe),
                    1,
                    "no home/closure alias survived teardown"
                );
                let cause = if mode == 0 { "returned" } else { "stopped" };
                let cut = recording.recorder.stop(cause, None).unwrap().unwrap().0;
                let projection = Projection::new(cause, &cut.progress, &recording.trace());
                let bytes = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink()
                    .bytes;
                drop(adapter);
                for replay_width in [1, 8] {
                    let replay = replay::Replay::new(&bytes).unwrap();
                    let install = replay.clone();
                    let (_, write) = UnixStream::pair().unwrap();
                    let probe = Arc::new(Probe {
                        run: Default::default(),
                        parked: false.into(),
                        parked_waiter: Default::default(),
                        delivered: false.into(),
                        done: false.into(),
                        home_dropped: false.into(),
                        waits: 0.into(),
                        write: std::sync::Mutex::new(write),
                        notice: Default::default(),
                        replay: true,
                        stop_in_wait: mode != 0,
                        expired_timer: matches!(mode, 3 | 4),
                    });
                    let mut run = crate::host::BodyRun::new(replay_width, false);
                    run.free = true;
                    unsafe {
                        run.use_physical_dispatch();
                    }
                    let outcome = run.run(
                        move |ctx| unsafe {
                            install.root(ctx);
                        },
                        move |ctx| unsafe {
                            let _role = role::Guard::new(ctx);
                            program(ctx, &probe, None);
                            replay::start(ctx);
                            unreachable!("root beyond replay cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(replay.finish().unwrap(), projection);
                }
            }
        }
    }
}
