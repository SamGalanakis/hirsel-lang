use super::*;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

struct Probe {
    run: AtomicPtr<crate::context::RunShared>,
    recording: Arc<super::super::record::Recording>,
    calls: AtomicU32,
    waits: AtomicU32,
    parks: AtomicU32,
    finishes: AtomicU32,
}
struct Home {
    thread: std::thread::ThreadId,
    probe: Arc<Probe>,
    available: bool,
    token: Option<u64>,
    advance: bool,
    affine: std::rc::Rc<()>,
}
impl Home {
    fn check(&self) {
        assert_eq!(std::thread::current().id(), self.thread);
        assert_eq!(std::rc::Rc::strong_count(&self.affine), 1);
        let guard = unsafe { role::IdleGuard::try_new(self.probe.run.load(Ordering::Acquire)) };
        assert!(guard.is_some(), "adapter call retains no role");
        drop(guard);
    }
}
impl crate::io::Adapter for Home {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn enter(&mut self, token: u64) {
        self.check();
        assert_eq!(*self.token.get_or_insert(token), token);
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        self.check();
        assert_eq!(
            request,
            Request::SleepUntil {
                clock: 17,
                deadline: 100
            }
        );
        assert!(self.token.is_some());
        // A synchronous cutter proves that no recorder section crosses the call.
        self.probe.recording.recorder.flush().unwrap().unwrap();
        let call = self.probe.calls.fetch_add(1, Ordering::AcqRel);
        if call < 2 {
            self.available = false;
            Err(AdapterError::Blocked)
        } else {
            assert_eq!(self.probe.waits.load(Ordering::Acquire), 2);
            Ok(Completion::Wait(Ok(())))
        }
    }
    fn parked(&mut self, token: u64) {
        self.check();
        assert_eq!(Some(token), self.token);
        self.probe.parks.fetch_add(1, Ordering::Release);
    }
    fn ready(&mut self) -> Result<crate::io::Ready, AdapterError> {
        self.check();
        Ok(crate::io::Ready::Tokens(
            if std::mem::take(&mut self.available) {
                self.token.into_iter().collect()
            } else {
                vec![]
            },
        ))
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        self.check();
        // There are no Hirsel tasks running while this fixture advances the
        // source. Every blocked observation and physical save already exists.
        self.probe.recording.recorder.flush().unwrap().unwrap();
        self.probe.waits.fetch_add(1, Ordering::Release);
        self.available = self.advance;
        Ok(self.advance)
    }
    fn finish(&mut self, token: u64) {
        self.check();
        assert_eq!(Some(token), self.token.take());
        self.probe.finishes.fetch_add(1, Ordering::Release);
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.thread);
    }
}

#[test]
fn native_home_readiness_retries_on_saved_owner_and_replays_without_adapter() {
    const CHILD: &str = "HIRSEL_F1_HOST_READINESS";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::host::readiness_tests::native_home_readiness_retries_on_saved_owner_and_replays_without_adapter", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(1);
    for width in [1, 8] {
        for durable in [false, true] {
            for advance in [false, true] {
                let recording =
                    super::super::record::Recording::new(b"native home readiness", durable);
                let install = recording.clone();
                let probe = Arc::new(Probe {
                    run: AtomicPtr::default(),
                    recording: recording.clone(),
                    calls: 0.into(),
                    waits: 0.into(),
                    parks: 0.into(),
                    finishes: 0.into(),
                });
                let observed = probe.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.adapter = Some(Box::new(Home {
                    thread: std::thread::current().id(),
                    probe: probe.clone(),
                    available: false,
                    token: None,
                    advance,
                    affine: std::rc::Rc::new(()),
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
                        let result = crate::stop::check(
                            ctx,
                            request_home(
                                ctx,
                                Request::SleepUntil {
                                    clock: 17,
                                    deadline: 100,
                                },
                                KeySet::coupled(),
                            ),
                        );
                        assert_eq!(result, Ok(Completion::Wait(Ok(()))));
                        assert_eq!((*ctx).run_ref().host_waits.load(Ordering::Acquire), 0);
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.pending, 0);
                assert_eq!(
                    probe.calls.load(Ordering::Acquire),
                    if advance { 3 } else { 1 }
                );
                assert_eq!(
                    probe.parks.load(Ordering::Acquire),
                    if advance { 2 } else { 1 }
                );
                assert_eq!(
                    probe.waits.load(Ordering::Acquire),
                    if advance { 2 } else { 1 }
                );
                assert_eq!(probe.finishes.load(Ordering::Acquire), u32::from(advance));
                if advance {
                    assert_eq!(outcome.ended, Ok(STATUS_OK));
                } else {
                    assert!(
                        matches!(outcome.ended, Err(Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                }
                drop(probe);
                let cut = recording
                    .recorder
                    .stop(if advance { "returned" } else { "blocked" }, None)
                    .unwrap()
                    .unwrap()
                    .0;
                let trace = recording.trace();
                let expected = hirsel_free::projection::Projection::new(
                    if advance { "returned" } else { "blocked" },
                    &cut.progress,
                    &trace,
                );
                let sink = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink();
                let parsed = hirsel_free::tape::parse(&sink.bytes).unwrap();
                let records = hirsel_free::record::decode(&parsed.logs[&Vec::new()]).unwrap();
                assert_eq!(
                    records
                        .iter()
                        .filter(|r| matches!(r, hirsel_free::record::Record::Blocked { .. }))
                        .count(),
                    if advance { 2 } else { 1 }
                );
                assert_eq!(
                    records
                        .iter()
                        .filter(|r| matches!(r, hirsel_free::record::Record::Ready { .. }))
                        .count(),
                    if advance { 2 } else { 0 }
                );
                let mut ends = sink.marks.clone();
                ends.push(sink.bytes.len());
                ends.sort_unstable();
                ends.dedup();
                for end in ends {
                    let parsed = hirsel_free::tape::parse(&sink.bytes[..end]).unwrap();
                    let expected_prefix = if end == sink.bytes.len() {
                        expected.clone()
                    } else {
                        hirsel_free::projection::Projection::new("truncated", &parsed.cut, &trace)
                    };
                    for replay_width in [1, 8] {
                        let replay = super::super::replay::Replay::new(&sink.bytes[..end]).unwrap();
                        let install = replay.clone();
                        let mut run = crate::host::BodyRun::new(replay_width, false);
                        run.free = true;
                        unsafe { run.use_physical_dispatch() };
                        let outcome = run.run(
                            move |ctx| unsafe { install.root(ctx) },
                            move |ctx| unsafe {
                                let _role = role::Guard::new(ctx);
                                let result = crate::stop::check(
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
                                assert!(advance);
                                assert_eq!(result, Ok(Completion::Wait(Ok(()))));
                                super::super::replay::start(ctx);
                                unreachable!("host readiness replay past terminal cut");
                            },
                        );
                        assert!(
                            matches!(outcome.ended, Err(Stop::Blocked(_))),
                            "{outcome:?}"
                        );
                        assert_eq!(replay.finish().unwrap(), expected_prefix);
                    }
                }
            }
        }
    }
}
