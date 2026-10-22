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

#[test]
fn native_home_cancellation_entry_validates_before_effect_and_preserves_consumed_cleanup() {
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
                run.adapter = Some(Box::new(Home {
                    thread: std::thread::current().id(),
                    probe: probe.clone(),
                    mode,
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
                        scheduler(ctx).cancel(0, 3);
                        if mode == 1 {
                            (*ctx).shield = 1;
                        }
                        let result = crate::stop::check(
                            ctx,
                            request_home(ctx, request(mode), KeySet::coupled()),
                        );
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
