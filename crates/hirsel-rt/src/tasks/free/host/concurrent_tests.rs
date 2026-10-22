//! Real ConcurrentIo through native Free phases, with replay metadata only.
use super::*;
use crate::io_concurrent::ConcurrentAdapter;
use crate::io_keys::{Kind, Registry};
use hirsel_free::projection::Projection;

#[derive(Default)]
struct NoSources(std::sync::atomic::AtomicU32, std::sync::atomic::AtomicU32);
impl ConcurrentAdapter for NoSources {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        Err(AdapterError::Blocked)
    }
    fn ready(&self) -> Result<crate::io::Ready, AdapterError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Release);
        Ok(crate::io::Ready::Tokens(vec![]))
    }
    fn finish(&self, _: u64) {
        self.1.fetch_add(1, std::sync::atomic::Ordering::Release);
    }
}

#[test]
fn concurrent_saved_request_without_external_sources_is_blocked_and_replays_its_prefix() {
    for width in [1, 8] {
        let adapter = Arc::new(NoSources::default());
        let mut admitted = Some(adapter.clone());
        let recording = super::super::record::Recording::new(b"concurrent no sources", false);
        let install = recording.clone();
        let mut run = crate::host::BodyRun::new(width, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                {
                    let _segment = role::Segment::new(ctx);
                    concurrent::Service::install(ctx, admitted.take().unwrap()).unwrap();
                }
                let _ = crate::stop::check(
                    ctx,
                    concurrent::request(
                        ctx,
                        Request::SleepUntil {
                            clock: 17,
                            deadline: 100,
                        },
                    ),
                );
                unreachable!("no source can complete this wait");
            },
        );
        assert_eq!(outcome.pending, 0);
        let Err(Stop::Blocked(waits)) = outcome.ended else {
            panic!("{outcome:?}")
        };
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].kind, "host request");
        assert!(
            adapter.0.load(std::sync::atomic::Ordering::Acquire) > 0,
            "home must prove the absence of sources"
        );
        assert_eq!(
            Arc::strong_count(&adapter),
            1,
            "stopped frame retained its concurrent service"
        );
        assert_eq!(
            adapter.1.load(std::sync::atomic::Ordering::Acquire),
            1,
            "stop teardown unregisters the outstanding token"
        );
        let (cut, sweep) = recording.recorder.stop("blocked", None).unwrap().unwrap();
        assert_eq!(sweep.in_doubt, [1]);
        assert_eq!(cut.progress[&Vec::new()], Progress::new(1, Phase::Ended(0)));
        let projection = Projection::new("blocked", &cut.progress, &recording.trace());
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
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
                    let _ = crate::stop::check(
                        ctx,
                        replay_request(
                            ctx,
                            Request::SleepUntil {
                                clock: 17,
                                deadline: 100,
                            },
                            KeySet::coupled(),
                        ),
                    );
                    unreachable!("replay has no admitted completion");
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

#[derive(Default)]
struct EarlyEdge {
    run: std::sync::atomic::AtomicPtr<crate::context::RunShared>,
    state: std::sync::Mutex<(Option<u64>, bool, bool)>,
    consumed: std::sync::Condvar,
    calls: std::sync::atomic::AtomicU32,
}
impl ConcurrentAdapter for EarlyEdge {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!()
    }
    fn call_at(&self, token: u64, _: Request) -> Result<Completion, AdapterError> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let mut state = self.state.lock().unwrap();
        if call == 0 {
            state.0 = Some(token);
            Err(AdapterError::Blocked)
        } else {
            assert_eq!(call, 1, "one owner-side retry");
            assert!(state.2, "edge was consumed before park returned");
            Ok(Completion::Wait(Ok(())))
        }
    }
    fn parked(&self, token: u64) {
        let mut state = self.state.lock().unwrap();
        assert_eq!(state.0, Some(token));
        state.1 = true;
        unsafe { crate::pool::wake_home(self.run.load(std::sync::atomic::Ordering::Acquire)) };
        let (state, timeout) = self
            .consumed
            .wait_timeout_while(state, std::time::Duration::from_secs(2), |state| !state.2)
            .unwrap();
        assert!(
            !timeout.timed_out() && state.2,
            "home must consume before physical save"
        );
    }
    fn ready(&self) -> Result<crate::io::Ready, AdapterError> {
        let mut state = self.state.lock().unwrap();
        if state.1 {
            state.1 = false;
            state.2 = true;
            self.consumed.notify_one();
            Ok(crate::io::Ready::Tokens(vec![state.0.unwrap()]))
        } else {
            Ok(crate::io::Ready::Tokens(vec![]))
        }
    }
    fn finish(&self, token: u64) {
        let mut state = self.state.lock().unwrap();
        assert_eq!(state.0.take(), Some(token));
    }
}

#[test]
fn concurrent_ready_edge_before_physical_save_is_latched_until_owner_retry() {
    for width in [1, 8] {
        let edge = Arc::new(EarlyEdge::default());
        let observed = edge.clone();
        let recording = super::super::record::Recording::new(b"early concurrent ready", false);
        let install = recording.clone();
        let mut run = crate::host::BodyRun::new(width, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let mut admitted = Some(edge.clone());
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                observed
                    .run
                    .store((*ctx).run_ptr(), std::sync::atomic::Ordering::Release);
                {
                    let _segment = role::Segment::new(ctx);
                    concurrent::Service::install(ctx, admitted.take().unwrap()).unwrap();
                }
                assert_eq!(
                    crate::stop::check(
                        ctx,
                        concurrent::request(
                            ctx,
                            Request::SleepUntil {
                                clock: 17,
                                deadline: 100
                            }
                        )
                    ),
                    Ok(Completion::Wait(Ok(())))
                );
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(edge.calls.load(std::sync::atomic::Ordering::Acquire), 2);
        let cut = recording
            .recorder
            .stop("returned", None)
            .unwrap()
            .unwrap()
            .0;
        let projection = Projection::new("returned", &cut.progress, &recording.trace());
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
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
                    let _ = crate::stop::check(
                        ctx,
                        replay_request(
                            ctx,
                            Request::SleepUntil {
                                clock: 17,
                                deadline: 100,
                            },
                            KeySet::coupled(),
                        ),
                    );
                    super::super::replay::start(ctx);
                    unreachable!("ready replay beyond cut");
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

struct Timer {
    real: crate::io_real::ConcurrentIo,
    parks: std::sync::atomic::AtomicU32,
}
impl ConcurrentAdapter for Timer {
    fn keys(&self, request: &Request) -> KeySet {
        self.real.keys(request)
    }
    fn identity(&self, word: u64) -> Option<(crate::io_keys::Key, Kind)> {
        self.real.identity(word)
    }
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        self.real.call_at(token, request)
    }
    fn call(&self, request: Request) -> Result<Completion, AdapterError> {
        self.real.call(request)
    }
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        self.real.preflight(request)
    }
    fn finish(&self, token: u64) {
        self.real.finish(token);
    }
    fn parked(&self, token: u64) {
        self.parks
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        self.real.parked(token);
    }
    fn cancellable(&self, token: u64) -> bool {
        self.real.cancellable(token)
    }
    fn ready(&self) -> Result<crate::io::Ready, AdapterError> {
        self.real.ready()
    }
    fn readiness(&self) -> Option<crate::readiness::Readiness> {
        self.real.readiness()
    }
}

unsafe fn sleep_program(ctx: *mut TaskCtx, adapter: Option<&dyn ConcurrentAdapter>) {
    let mut registry = Registry::default();
    let Completion::ClockGrant(Ok(clock)) = (unsafe {
        request(
            ctx,
            Request::MonotonicClock { io: 1 },
            adapter,
            &mut registry,
        )
    }) else {
        panic!("clock grant")
    };
    let Completion::Monotonic(Ok(now)) =
        (unsafe { request(ctx, Request::Monotonic { clock }, adapter, &mut registry) })
    else {
        panic!("clock time")
    };
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::SleepUntil {
                    clock,
                    deadline: now + 20_000_000,
                },
                adapter,
                &mut registry,
            )
        },
        Completion::Wait(Ok(()))
    );
}

#[test]
fn concurrent_real_timer_saves_owner_retries_and_replays_without_driver() {
    for width in [1, 8] {
        for durable in [false, true] {
            let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
            real.discard_history();
            real.clock_grants(true, false);
            let adapter = Arc::new(Timer {
                real: real
                    .into_concurrent()
                    .unwrap_or_else(|_| panic!("Send timer")),
                parks: Default::default(),
            });
            let recording = super::super::record::Recording::new(b"concurrent timer", durable);
            let install = recording.clone();
            let mut admitted = Some(adapter.clone());
            let observed = adapter.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    {
                        let _segment = role::Segment::new(ctx);
                        concurrent::Service::install(ctx, admitted.take().unwrap()).unwrap();
                    }
                    sleep_program(ctx, Some(&*observed));
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            assert!(adapter.parks.load(std::sync::atomic::Ordering::Acquire) > 0);
            drop(adapter);
            let cut = recording
                .recorder
                .stop("returned", None)
                .unwrap()
                .unwrap()
                .0;
            let projection = Projection::new("returned", &cut.progress, &recording.trace());
            let bytes = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink()
                .bytes;
            for replay_width in [1, 8] {
                let replay = super::super::replay::Replay::new(&bytes).unwrap();
                let install = replay.clone();
                let mut run = crate::host::BodyRun::new(replay_width, false);
                run.free = true;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        let _role = role::Guard::new(ctx);
                        sleep_program(ctx, None);
                        super::super::replay::start(ctx);
                        unreachable!("timer replay beyond cut");
                    },
                );
                assert_eq!(outcome.pending, 0);
                assert!(
                    matches!(outcome.ended, Err(Stop::Blocked(_))),
                    "{outcome:?}"
                );
                assert_eq!(projection, replay.finish().unwrap());
            }
        }
    }
}

unsafe fn request(
    ctx: *mut TaskCtx,
    request: Request,
    adapter: Option<&dyn ConcurrentAdapter>,
    registry: &mut Registry,
) -> Completion {
    let mut result = unsafe {
        if let Some(adapter) = adapter {
            if (*ctx).run_ref().free_io.get().is_some() {
                crate::stop::check(ctx, concurrent::request(ctx, request.clone()))
            } else {
                crate::stop::check(
                    ctx,
                    prepare_with_keys(ctx, request.clone(), adapter.keys(&request)),
                );
                assert!(crate::stop::check(ctx, claim(ctx)));
                let token = (*ctx).pending_host.as_ref().unwrap().request.id;
                crate::stop::check(ctx, invoke_concurrent(ctx, adapter));
                crate::stop::check(ctx, ended(ctx));
                let result = crate::stop::check(ctx, complete_result(ctx));
                let _segment = role::Segment::new(ctx);
                adapter.finish(token);
                result
            }
        } else {
            crate::stop::check(
                ctx,
                replay_request(ctx, request.clone(), registry.keys(&request)),
            )
        }
    };
    for word in handles::fields(&mut result).unwrap().into_iter().flatten() {
        let (key, kind) = if let Some(adapter) = adapter {
            adapter.identity(*word).expect("registered live result")
        } else {
            crate::stop::check(ctx, replay_created_identity(ctx, *word))
        };
        registry.bind(*word, key, kind);
    }
    result.unwrap()
}

unsafe fn file_program(ctx: *mut TaskCtx, adapter: Option<&dyn ConcurrentAdapter>) {
    let mut registry = Registry::default();
    let directory = unsafe {
        request(
            ctx,
            Request::Directory {
                io: 1,
                path: String::new(),
            },
            adapter,
            &mut registry,
        )
    };
    let Completion::Handle(Ok(directory)) = directory else {
        panic!("directory")
    };
    let file = unsafe {
        request(
            ctx,
            Request::Create {
                directory,
                path: "data".into(),
            },
            adapter,
            &mut registry,
        )
    };
    let Completion::Handle(Ok(file)) = file else {
        panic!("file")
    };
    let (resource, kind) = registry.identity(file).unwrap();
    assert_eq!(kind, Kind::File);
    assert_ne!(resource, crate::io_keys::COUPLED);
    let bytes = b"real concurrent host result\0\xff".to_vec();
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::FileWrite {
                    directory,
                    file,
                    bytes: bytes.clone(),
                },
                adapter,
                &mut registry,
            )
        },
        Completion::Size(Ok(bytes.len() as u64))
    );
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::FileSeek {
                    directory,
                    file,
                    offset: 0,
                },
                adapter,
                &mut registry,
            )
        },
        Completion::Unit(Ok(()))
    );
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::FileRead {
                    directory,
                    file,
                    max: 4096,
                },
                adapter,
                &mut registry,
            )
        },
        Completion::Bytes(Err(crate::io::IoError::Fault(1)))
    );
    let alias = unsafe {
        request(
            ctx,
            Request::Open {
                directory,
                path: "data".into(),
            },
            adapter,
            &mut registry,
        )
    };
    let Completion::Handle(Ok(alias)) = alias else {
        panic!("file alias")
    };
    assert_ne!(file, alias);
    assert_eq!(registry.identity(alias), Some((resource, Kind::File)));
    assert_eq!(
        unsafe { request(ctx, Request::Close { handle: file }, adapter, &mut registry) },
        Completion::Unit(Ok(()))
    );
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::FileRead {
                    directory,
                    file: alias,
                    max: 4096,
                },
                adapter,
                &mut registry,
            )
        },
        Completion::Bytes(Ok(bytes.clone()))
    );
    let mapped = unsafe {
        request(
            ctx,
            Request::Map {
                directory,
                path: "data".into(),
            },
            adapter,
            &mut registry,
        )
    };
    let Completion::Handle(Ok(mapped)) = mapped else {
        panic!("mapping")
    };
    assert_eq!(registry.identity(mapped), Some((resource, Kind::File)));
    let view = unsafe {
        request(
            ctx,
            Request::MapView { directory, mapped },
            adapter,
            &mut registry,
        )
    };
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::Close { handle: mapped },
                adapter,
                &mut registry,
            )
        },
        Completion::Unit(Ok(()))
    );
    let Completion::View(view) = view else {
        panic!("view")
    };
    assert_eq!(view.block, 0);
    assert_eq!(view.contents(), bytes);
    assert_eq!(
        unsafe {
            request(
                ctx,
                Request::Close { handle: alias },
                adapter,
                &mut registry,
            )
        },
        Completion::Unit(Ok(()))
    );
}

#[test]
fn real_concurrent_files_discover_aliases_and_replay_without_mount_or_adapter() {
    for durable in [false, true] {
        let path = std::env::temp_dir().join(format!(
            "hirsel-f1-real-concurrent-{}-{durable}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        let mut real = crate::io_real::RealIo::grant(&path).unwrap();
        real.discard_history();
        let io = Arc::new(
            real.into_concurrent()
                .unwrap_or_else(|_| panic!("built-in Send adapter")),
        );
        let recording =
            super::super::record::Recording::new(b"real concurrent free files", durable);
        let install = recording.clone();
        let observed = io.clone();
        let mut run = crate::host::BodyRun::new(8, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                super::super::enable(ctx);
                install.root(ctx);
            },
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                file_program(ctx, Some(&*observed));
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(
            std::fs::read(path.join("data")).unwrap(),
            b"real concurrent host result\0\xff"
        );
        drop(io);
        std::fs::remove_dir_all(path).unwrap();
        let cut = recording
            .recorder
            .stop("returned", None)
            .unwrap()
            .unwrap()
            .0;
        let projection = Projection::new("returned", &cut.progress, &recording.trace());
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
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
                    file_program(ctx, None);
                    super::super::replay::start(ctx);
                    unreachable!("real file replay past cut");
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
