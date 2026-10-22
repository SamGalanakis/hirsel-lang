use super::*;
use hirsel_free::claim::Sweep;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

struct Probe {
    recording: Arc<super::super::record::Recording>,
    sweep: Mutex<Option<Sweep>>,
    request: Mutex<Option<std::sync::Weak<hirsel_free::claim::Request<Request>>>>,
    issued: AtomicBool,
    delivered: AtomicBool,
}

fn cut(probe: &Probe) {
    let (_, sweep) = probe
        .recording
        .recorder
        .stop("stopped", None)
        .unwrap()
        .unwrap();
    *probe.sweep.lock().unwrap() = Some(sweep);
}

#[test]
fn native_host_claim_withdraw_and_quarantine_preserve_owned_buffers() {
    for width in [1, 8] {
        for durable in [false, true] {
            // Stop before claim, after claim before call, inside the call,
            // after call/Ended before Done, or let the request complete.
            for point in 0..=4 {
                let recording =
                    super::super::record::Recording::new(b"native host claim matrix", durable);
                let install = recording.clone();
                let probe = Arc::new(Probe {
                    recording: recording.clone(),
                    sweep: Mutex::new(None),
                    request: Mutex::new(None),
                    issued: false.into(),
                    delivered: false.into(),
                });
                let observed = probe.clone();
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
                        crate::stop::check(
                            ctx,
                            prepare(
                                ctx,
                                Request::Print {
                                    stdout: 17,
                                    bytes: vec![42; 4096],
                                },
                            ),
                        );
                        {
                            let pending = (*ctx).pending_host.as_deref().unwrap();
                            *observed.request.lock().unwrap() =
                                Some(Arc::downgrade(&pending.request));
                        }
                        if point == 0 {
                            cut(&observed);
                        }
                        let claimed = claim(ctx);
                        if point == 0 {
                            assert!(matches!(claimed, Err(Stop::Requested) | Ok(false)));
                            crate::stop::check(ctx, Err::<(), _>(Stop::Requested));
                            unreachable!();
                        }
                        assert!(crate::stop::check(ctx, claimed));
                        if point == 1 {
                            cut(&observed);
                        }
                        crate::stop::check(
                            ctx,
                            invoke(ctx, |request| {
                                let Request::Print { stdout, bytes } = request else {
                                    panic!("typed request buffers")
                                };
                                assert_eq!(*stdout, 17);
                                assert_eq!(bytes, &vec![42; 4096]);
                                observed.issued.store(true, Ordering::Release);
                                if point == 2 {
                                    cut(&observed);
                                }
                                // The cut cannot free borrowed request storage.
                                assert_eq!(bytes.len(), 4096);
                                assert_eq!(bytes[4095], 42);
                                Completion::Unit(Ok(()))
                            }),
                        );
                        let ended = ended(ctx);
                        if point == 1 || point == 2 {
                            assert!(matches!(ended, Err(Stop::Requested)));
                            assert!(
                                (*ctx).pending_host.as_ref().unwrap().completion.is_some(),
                                "late completion remains quarantined in task storage"
                            );
                            crate::stop::check(ctx, ended);
                            unreachable!();
                        }
                        crate::stop::check(ctx, ended);
                        if point == 3 {
                            cut(&observed);
                        }
                        let completion = complete(ctx);
                        if point == 3 {
                            assert!(matches!(completion, Err(Stop::Requested)));
                            crate::stop::check(ctx, completion);
                            unreachable!();
                        }
                        assert_eq!(
                            crate::stop::check(ctx, completion),
                            Completion::Unit(Ok(()))
                        );
                        assert!((*ctx).pending_host.is_none());
                        observed.delivered.store(true, Ordering::Release);
                        STATUS_OK
                    },
                );
                assert_eq!(probe.issued.load(Ordering::Acquire), point > 0);
                assert_eq!(probe.delivered.load(Ordering::Acquire), point == 4);
                assert!(
                    probe
                        .request
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .upgrade()
                        .is_none(),
                    "the request's last buffer owner left at completion or task teardown"
                );
                if point == 4 {
                    assert_eq!(outcome.ended, Ok(STATUS_OK));
                    recording.recorder.stop("returned", None).unwrap().unwrap();
                } else {
                    assert_eq!(outcome.ended, Err(Stop::Requested));
                    let sweep = probe.sweep.lock().unwrap();
                    let sweep = sweep.as_ref().unwrap();
                    assert_eq!(sweep.withdrawn, if point == 0 { vec![1] } else { vec![] });
                    assert_eq!(sweep.in_doubt, if point == 0 { vec![] } else { vec![1] });
                }
                drop(probe);
                let bytes = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink()
                    .bytes;
                let tape = hirsel_free::tape::parse(&bytes).unwrap();
                assert_eq!(
                    tape.cut[&Vec::new()],
                    Progress::new(
                        1,
                        match point {
                            0 => Phase::Entered,
                            1 | 2 => Phase::Claimed,
                            3 => Phase::Ended(0),
                            _ => Phase::Done,
                        }
                    )
                );
            }
        }
    }
}

struct EpochProbe {
    go: AtomicBool,
    entered: std::sync::atomic::AtomicU32,
    done: std::sync::atomic::AtomicU32,
    calls: Mutex<Vec<u32>>,
}

unsafe fn yield_until(ctx: *mut TaskCtx, label: &str, mut ready: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !ready() {
        assert!(
            std::time::Instant::now() < until,
            "host epoch progress: {label}"
        );
        // Test stage changes explicitly wake Retry waiters; repeatedly
        // enqueueing this fiber locally can starve untouched injection work
        // at width one before any host phase has even started.
        park_retry(ctx, false);
    }
}

unsafe extern "C" fn epoch_child(ctx: *mut u8, data: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let probe = unsafe { &*data.cast::<EpochProbe>() };
    let outer_role = unsafe { role::Guard::new(ctx) };
    let id = unsafe { (*ctx).task_id };
    unsafe {
        yield_until(ctx, "child enter order", || {
            probe.go.load(Ordering::Acquire) && probe.entered.load(Ordering::Acquire) == id - 1
        });
        crate::stop::check(
            ctx,
            prepare_with_keys(
                ctx,
                Request::Print {
                    stdout: 17,
                    bytes: vec![id as u8],
                },
                KeySet::new(vec![
                    (crate::io_keys::Key(18), Mode::Shared),
                    (
                        crate::io_keys::Key(17),
                        if id == 2 {
                            Mode::Exclusive
                        } else {
                            Mode::Shared
                        },
                    ),
                ]),
            ),
        );
        probe.entered.store(id, Ordering::Release);
        wake_retrying(ctx);
        // The first shared member may issue alongside root; do not let it
        // finish until all later positions, including an exclusive, exist.
        if id == 1 {
            yield_until(ctx, "shared member sees later entries", || {
                probe.entered.load(Ordering::Acquire) == 3
            });
        }
        assert!(crate::stop::check(ctx, claim(ctx)));
        crate::stop::check(
            ctx,
            invoke(ctx, |_| {
                probe.calls.lock().unwrap().push(id);
                Completion::Unit(Ok(()))
            }),
        );
        crate::stop::check(ctx, ended(ctx));
        crate::stop::check(ctx, complete(ctx));
        probe.done.fetch_add(1, Ordering::Release);
        wake_retrying(ctx);
        drop(outer_role);
        role::enter(ctx);
        crate::pool::free::finish_current(ctx);
    }
}

#[test]
fn native_host_epochs_release_workers_and_end_before_readiness_attempts() {
    for width in [1, 8] {
        for durable in [false, true] {
            let recording = super::super::record::Recording::new(b"native key epochs", durable);
            let install = recording.clone();
            let probe = Arc::new(EpochProbe {
                go: false.into(),
                entered: 0.into(),
                done: 0.into(),
                calls: Mutex::new(Vec::new()),
            });
            let observed = probe.clone();
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
                    // All request owners precede their first save. Raw dispatch
                    // children use only host phases and explicit physical saves.
                    for _ in 0..3 {
                        let stack = Stack::new();
                        let child_ctx = stack.context();
                        child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
                        let child = crate::stop::check(ctx, super::super::record::child(ctx));
                        let path = child.path().to_vec();
                        (*child_ctx).recording = Some(Box::new(child));
                        let s = scheduler(ctx);
                        let id = s.tasks.len() as u32;
                        (*child_ctx).task_id = id;
                        (*child_ctx).task_sp = stack.prepare(
                            epoch_child,
                            child_ctx.cast(),
                            Arc::as_ptr(&observed).cast_mut().cast(),
                        );
                        let mut row = crate::tasks::Task::new(1, child_ctx, None, path, None);
                        row.stack = Some(stack);
                        s.tasks.push(Some(row));
                        crate::pool::free::schedule_saved((*ctx).run_ptr(), id, Owner::Any);
                    }
                    crate::stop::check(
                        ctx,
                        prepare_with_keys(
                            ctx,
                            Request::Print {
                                stdout: 17,
                                bytes: vec![0],
                            },
                            KeySet::new(vec![
                                (crate::io_keys::Key(18), Mode::Shared),
                                (crate::io_keys::Key(17), Mode::Shared),
                            ]),
                        ),
                    );
                    assert!(crate::stop::check(ctx, claim(ctx)));
                    observed.go.store(true, Ordering::Release);
                    wake_retrying(ctx);
                    yield_until(ctx, "root waits for later key entries", || {
                        observed.entered.load(Ordering::Acquire) == 3
                            && observed.done.load(Ordering::Acquire) >= 1
                    });
                    // Entry metadata can precede the physical idle save.
                    // Release the role while those idle continuations finish;
                    // no worker/permit is needed to finish an already saved frame.
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    loop {
                        let s = scheduler(ctx);
                        if [2, 3].into_iter().all(|id| {
                            matches!(s.task(id).wait, Some(Wait::Retry { .. }))
                                && super::super::super::readiness::saved(s, id)
                        }) {
                            break;
                        }
                        assert!(
                            std::time::Instant::now() < until,
                            "key waiters physically saved"
                        );
                        let _segment = role::Segment::new(ctx);
                        std::thread::yield_now();
                    }
                    assert!(observed.calls.lock().unwrap().iter().all(|id| *id == 1));
                    crate::stop::check(ctx, invoke_result(ctx, |_| Err(AdapterError::Blocked)));
                    crate::stop::check(ctx, ended(ctx));
                    assert!((*ctx).pending_host.as_ref().unwrap().positions.is_none());
                    // The exclusive and its following shared epoch must finish
                    // while root's request still waits for readiness, not Done.
                    yield_until(ctx, "root waits for calls before readiness", || {
                        observed.done.load(Ordering::Acquire) == 3
                    });
                    assert_eq!(*observed.calls.lock().unwrap(), [1, 2, 3]);
                    crate::stop::check(ctx, attempt(ctx));
                    assert_eq!((*ctx).pending_host.as_ref().unwrap().attempt, 1);
                    crate::stop::check(ctx, invoke(ctx, |_| Completion::Unit(Ok(()))));
                    crate::stop::check(ctx, ended(ctx));
                    crate::stop::check(ctx, complete(ctx));
                    STATUS_OK
                },
            );
            assert_eq!(
                outcome.ended,
                Ok(STATUS_OK),
                "width={width} durable={durable} go={} entered={} done={} calls={:?} trace={:?}",
                probe.go.load(Ordering::Acquire),
                probe.entered.load(Ordering::Acquire),
                probe.done.load(Ordering::Acquire),
                probe.calls.lock().unwrap(),
                recording.trace()
            );
            assert_eq!(outcome.pending, 0);
            let cut = recording
                .recorder
                .stop("returned", None)
                .unwrap()
                .unwrap()
                .0;
            let trace = recording.trace();
            let projection =
                hirsel_free::projection::Projection::new("returned", &cut.progress, &trace);
            let epochs = &projection.keys["host-resource/17"];
            assert_eq!(epochs.len(), 3);
            assert_eq!(
                epochs[&1].len(),
                2,
                "shared epoch members issue independently"
            );
            assert_eq!(epochs[&2], vec![(vec![1], 1, 0)]);
            assert_eq!(
                epochs[&3].len(),
                2,
                "attempt joins the following shared epoch"
            );
            assert!(epochs[&3].contains(&(vec![], 4, 1)));
            assert_eq!(projection.keys["host-resource/18"].len(), 1);
            assert_eq!(
                projection.keys["host-resource/18"][&1].len(),
                5,
                "the independent shared key never couples this key's members"
            );
            let bytes = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink()
                .bytes;
            let parsed = hirsel_free::tape::parse(&bytes).unwrap();
            assert_eq!(parsed.cut, cut.progress);
            replay_epoch_tape(&bytes, &projection);
        }
    }
}

struct ConcurrentHost {
    home: std::thread::ThreadId,
    recording: Arc<super::super::record::Recording>,
    run: std::sync::atomic::AtomicPtr<crate::context::RunShared>,
    calls: std::sync::atomic::AtomicU32,
    stop_inside: bool,
    retry: bool,
    refuse: bool,
}

impl crate::io_concurrent::ConcurrentAdapter for ConcurrentHost {
    fn keys(&self, _: &Request) -> KeySet {
        KeySet::coupled()
    }
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&self, request: Request) -> Result<Completion, AdapterError> {
        assert_ne!(
            std::thread::current().id(),
            self.home,
            "direct adapter stays on compute"
        );
        let guard = unsafe { role::IdleGuard::try_new(self.run.load(Ordering::Acquire)) };
        assert!(guard.is_some(), "bounded call retains no scheduler role");
        drop(guard);
        let Request::Print { stdout: 17, bytes } = request else {
            panic!("typed request")
        };
        assert_eq!(bytes, vec![42; 4096]);
        let call = self.calls.fetch_add(1, Ordering::AcqRel);
        // The call can cut synchronously: there is no recorder section alive.
        if self.stop_inside {
            let (cut, sweep) = self
                .recording
                .recorder
                .stop("stopped", None)
                .unwrap()
                .unwrap();
            assert_eq!(cut.progress[&Vec::new()], Progress::new(1, Phase::Claimed));
            assert_eq!(sweep.in_doubt, [1]);
            assert!(sweep.withdrawn.is_empty());
            assert_eq!(bytes[4095], 42, "cut retains the adapter's owned buffers");
        } else {
            // A claimed call may start after the terminal cut; a closed
            // recorder correctly returns no further flush.
            let _ = self.recording.recorder.flush().unwrap();
        }
        if self.retry && call == 0 {
            Err(AdapterError::Blocked)
        } else if self.refuse {
            Err(AdapterError::Unsupported("native refusal".into()))
        } else {
            Ok(Completion::Unit(Ok(())))
        }
    }
}

#[test]
fn native_concurrent_adapter_observations_retry_refusal_and_late_completion() {
    use crate::io_concurrent::ConcurrentAdapter;
    for width in [1, 8] {
        for durable in [false, true] {
            for kind in 0..7 {
                let recording =
                    super::super::record::Recording::new(b"concurrent host result", durable);
                let install = recording.clone();
                let adapter = Arc::new(ConcurrentHost {
                    home: std::thread::current().id(),
                    recording: recording.clone(),
                    run: Default::default(),
                    calls: 0.into(),
                    stop_inside: kind == 3,
                    retry: kind == 1,
                    refuse: kind == 2,
                });
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
                        observed.run.store((*ctx).run_ptr(), Ordering::Release);
                        let request = Request::Print {
                            stdout: 17,
                            bytes: vec![42; 4096],
                        };
                        assert_eq!(observed.preflight(&request), Ok(None));
                        let keys = observed.keys(&request);
                        crate::stop::check(ctx, prepare_with_keys(ctx, request, keys));
                        if kind == 4 {
                            let (_, sweep) = observed
                                .recording
                                .recorder
                                .stop("stopped", None)
                                .unwrap()
                                .unwrap();
                            assert_eq!(sweep.withdrawn, [1]);
                            assert!(sweep.in_doubt.is_empty());
                            let claimed = claim(ctx);
                            assert!(matches!(claimed, Err(Stop::Requested) | Ok(false)));
                            crate::stop::check(ctx, Err::<(), _>(Stop::Requested));
                            unreachable!();
                        }
                        assert!(crate::stop::check(ctx, claim(ctx)));
                        if kind == 5 {
                            let (_, sweep) = observed
                                .recording
                                .recorder
                                .stop("stopped", None)
                                .unwrap()
                                .unwrap();
                            assert_eq!(sweep.in_doubt, [1]);
                            assert!(sweep.withdrawn.is_empty());
                        }
                        crate::stop::check(ctx, invoke_concurrent(ctx, &*observed));
                        crate::stop::check(ctx, ended(ctx));
                        if kind == 1 {
                            assert_eq!(
                                (*ctx).pending_host.as_ref().unwrap().completion,
                                Some(Err(AdapterError::Blocked))
                            );
                            assert!((*ctx).pending_host.as_ref().unwrap().positions.is_none());
                            crate::stop::check(ctx, attempt(ctx));
                            crate::stop::check(ctx, invoke_concurrent(ctx, &*observed));
                            crate::stop::check(ctx, ended(ctx));
                        }
                        if kind == 6 {
                            let (_, sweep) = observed
                                .recording
                                .recorder
                                .stop("stopped", None)
                                .unwrap()
                                .unwrap();
                            assert_eq!(sweep.in_doubt, [1]);
                            assert!(sweep.withdrawn.is_empty());
                        }
                        let result = crate::stop::check(ctx, complete_result(ctx));
                        assert_eq!(
                            result,
                            if kind == 2 {
                                Err(AdapterError::Unsupported("native refusal".into()))
                            } else {
                                Ok(Completion::Unit(Ok(())))
                            }
                        );
                        STATUS_OK
                    },
                );
                assert_eq!(
                    adapter.calls.load(Ordering::Acquire),
                    if kind == 4 {
                        0
                    } else if kind == 1 {
                        2
                    } else {
                        1
                    }
                );
                if kind >= 3 {
                    assert_eq!(outcome.ended, Err(Stop::Requested));
                } else {
                    assert_eq!(outcome.ended, Ok(STATUS_OK));
                    recording.recorder.stop("returned", None).unwrap().unwrap();
                }
                drop(adapter);
                let bytes = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink()
                    .bytes;
                let parsed = hirsel_free::tape::parse(&bytes).unwrap();
                let records = hirsel_free::record::decode(&parsed.logs[&Vec::new()]).unwrap();
                let observations: Vec<Result<Completion, AdapterError>> = records
                    .iter()
                    .filter_map(|record| {
                        if let hirsel_free::record::Record::Observation { bytes, .. } = record {
                            Some(crate::tape_wire::decode::<Envelope>(bytes).unwrap().result)
                        } else {
                            None
                        }
                    })
                    .collect();
                let completed: Vec<Result<Completion, AdapterError>> = records
                    .iter()
                    .filter_map(|record| {
                        if let hirsel_free::record::Record::Completion { bytes, .. } = record {
                            Some(crate::tape_wire::decode::<Envelope>(bytes).unwrap().result)
                        } else {
                            None
                        }
                    })
                    .collect();
                if kind >= 3 {
                    assert_eq!(observations.len(), usize::from(kind == 6));
                    assert!(completed.is_empty());
                    assert_eq!(
                        parsed.cut[&Vec::new()],
                        Progress::new(
                            1,
                            match kind {
                                4 => Phase::Entered,
                                6 => Phase::Ended(0),
                                _ => Phase::Claimed,
                            }
                        )
                    );
                } else {
                    let expected = if kind == 2 {
                        Err(AdapterError::Unsupported("native refusal".into()))
                    } else {
                        Ok(Completion::Unit(Ok(())))
                    };
                    assert_eq!(completed, vec![expected.clone()]);
                    assert_eq!(
                        observations,
                        if kind == 1 {
                            vec![Err(AdapterError::Blocked), expected]
                        } else {
                            vec![expected]
                        }
                    );
                }
            }
        }
    }
}

struct HomeHost {
    home: std::thread::ThreadId,
    run: Arc<std::sync::atomic::AtomicPtr<crate::context::RunShared>>,
    recording: Arc<super::super::record::Recording>,
    output: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    dropped: std::rc::Rc<std::cell::Cell<bool>>,
    stop_inside: bool,
}
impl crate::io::Adapter for HomeHost {
    fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
        Ok(None)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(
            std::thread::current().id(),
            self.home,
            "non-Send adapter stays home"
        );
        let guard = unsafe { role::IdleGuard::try_new(self.run.load(Ordering::Acquire)) };
        assert!(guard.is_some(), "home request retains no scheduler role");
        drop(guard);
        let Request::Print { stdout: 17, bytes } = request else {
            panic!("home request")
        };
        if self.stop_inside {
            let (cut, sweep) = self
                .recording
                .recorder
                .stop("stopped", None)
                .unwrap()
                .unwrap();
            assert_eq!(cut.progress[&Vec::new()], Progress::new(1, Phase::Claimed));
            assert_eq!(sweep.in_doubt, [1]);
        } else {
            let _ = self.recording.recorder.flush().unwrap();
        }
        assert_eq!(bytes, vec![42; 4096]);
        self.output.borrow_mut().extend_from_slice(&bytes);
        Ok(Completion::Unit(Ok(())))
    }
}
impl Drop for HomeHost {
    fn drop(&mut self) {
        assert_eq!(
            std::thread::current().id(),
            self.home,
            "adapter destruction stays home"
        );
        self.dropped.set(true);
    }
}

#[test]
fn native_home_claim_uses_owned_log_and_quarantines_non_send_adapter_results() {
    for width in [1, 8] {
        for durable in [false, true] {
            for point in 0..=5 {
                let recording = super::super::record::Recording::new(b"home host claims", durable);
                let install = recording.clone();
                let observed = recording.clone();
                let output = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                let dropped = std::rc::Rc::new(std::cell::Cell::new(false));
                let run_ptr = Arc::new(std::sync::atomic::AtomicPtr::default());
                let worker_run = run_ptr.clone();
                let request_weak = Arc::new(Mutex::new(
                    None::<std::sync::Weak<hirsel_free::claim::Request<Request>>>,
                ));
                let weak = request_weak.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                run.adapter = Some(Box::new(HomeHost {
                    home: std::thread::current().id(),
                    run: run_ptr,
                    recording: recording.clone(),
                    output: output.clone(),
                    dropped: dropped.clone(),
                    stop_inside: point == 2,
                }));
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        let _role = role::Guard::new(ctx);
                        worker_run.store((*ctx).run_ptr(), Ordering::Release);
                        crate::stop::check(
                            ctx,
                            prepare(
                                ctx,
                                Request::Print {
                                    stdout: 17,
                                    bytes: vec![42; 4096],
                                },
                            ),
                        );
                        *weak.lock().unwrap() = Some(Arc::downgrade(
                            &(*ctx).pending_host.as_ref().unwrap().request,
                        ));
                        if point == 0 {
                            let (_, sweep) =
                                observed.recorder.stop("stopped", None).unwrap().unwrap();
                            assert_eq!(sweep.withdrawn, [1]);
                            assert!(sweep.in_doubt.is_empty());
                        }
                        let issued = if point == 5 {
                            let shared = &observed;
                            let run_ptr = (*ctx).run_ptr();
                            issue_home_loan(
                                ctx,
                                || {
                                    // The transport has posted and admitted this
                                    // callback on home, but native issue is still Prepared.
                                    let (cut, sweep) =
                                        shared.recorder.stop("stopped", None).unwrap().unwrap();
                                    assert_eq!(
                                        cut.progress[&Vec::new()],
                                        Progress::new(1, Phase::Entered)
                                    );
                                    assert_eq!(sweep.withdrawn, [1]);
                                    assert!(sweep.in_doubt.is_empty());
                                },
                                |request| {
                                    (&mut *(*run_ptr).io_binding().cast::<crate::io::Binding>())
                                        .adapter
                                        .call(request.clone())
                                },
                            )
                        } else if point == 1 {
                            let shared = &observed;
                            let run_ptr = (*ctx).run_ptr();
                            issue_home_with(ctx, |request| {
                                // Home has published Claimed, but has not yet
                                // entered the adapter. A cut cannot withdraw it.
                                let (_, sweep) =
                                    shared.recorder.stop("stopped", None).unwrap().unwrap();
                                assert_eq!(sweep.in_doubt, [1]);
                                assert!(sweep.withdrawn.is_empty());
                                (&mut *(*run_ptr).io_binding().cast::<crate::io::Binding>())
                                    .adapter
                                    .call(request.clone())
                            })
                        } else {
                            issue_home(ctx)
                        };
                        assert!(
                            (*ctx).recording.is_some(),
                            "worker regains the owned log before any stop"
                        );
                        assert!((*ctx).pending_host.as_ref().unwrap().home_log.is_none());
                        if point == 0 || point == 5 {
                            assert!(matches!(issued, Err(Stop::Requested) | Ok(false)));
                            crate::stop::check(ctx, Err::<(), _>(Stop::Requested));
                            unreachable!();
                        }
                        assert!(crate::stop::check(ctx, issued));
                        crate::stop::check(ctx, ended(ctx));
                        if point == 3 {
                            let (_, sweep) =
                                observed.recorder.stop("stopped", None).unwrap().unwrap();
                            assert_eq!(sweep.in_doubt, [1]);
                        }
                        assert_eq!(
                            crate::stop::check(ctx, complete(ctx)),
                            Completion::Unit(Ok(()))
                        );
                        STATUS_OK
                    },
                );
                assert!(dropped.get());
                assert_eq!(
                    output.borrow().len(),
                    if point == 0 || point == 5 { 0 } else { 4096 }
                );
                assert!(
                    request_weak
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .upgrade()
                        .is_none()
                );
                if point == 4 {
                    assert_eq!(outcome.ended, Ok(STATUS_OK));
                    recording.recorder.stop("returned", None).unwrap().unwrap();
                } else {
                    assert_eq!(outcome.ended, Err(Stop::Requested));
                }
                let bytes = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink()
                    .bytes;
                let tape = hirsel_free::tape::parse(&bytes).unwrap();
                assert_eq!(
                    tape.cut[&Vec::new()],
                    Progress::new(
                        1,
                        match point {
                            0 | 5 => Phase::Entered,
                            1 | 2 => Phase::Claimed,
                            3 => Phase::Ended(0),
                            _ => Phase::Done,
                        }
                    )
                );
            }
        }
    }
}

#[test]
fn native_host_replay_substitutes_adapter_results_and_stops_at_partial_phases() {
    for durable in [false, true] {
        for retry in [false, true] {
            for refuse in [false, true] {
                let recording =
                    super::super::record::Recording::new(b"native host replay", durable);
                let install = recording.clone();
                let adapter = Arc::new(ConcurrentHost {
                    home: std::thread::current().id(),
                    recording: recording.clone(),
                    run: Default::default(),
                    calls: 0.into(),
                    stop_inside: false,
                    retry,
                    refuse,
                });
                let observed = adapter.clone();
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
                        observed.run.store((*ctx).run_ptr(), Ordering::Release);
                        crate::stop::check(
                            ctx,
                            prepare(
                                ctx,
                                Request::Print {
                                    stdout: 17,
                                    bytes: vec![42; 4096],
                                },
                            ),
                        );
                        assert!(crate::stop::check(ctx, claim(ctx)));
                        crate::stop::check(ctx, invoke_concurrent(ctx, &*observed));
                        crate::stop::check(ctx, ended(ctx));
                        // Keep an Ended cut as well as the adapter's Claimed cut.
                        observed.recording.recorder.flush().unwrap().unwrap();
                        if retry {
                            crate::stop::check(ctx, attempt(ctx));
                            crate::stop::check(ctx, invoke_concurrent(ctx, &*observed));
                            crate::stop::check(ctx, ended(ctx));
                        }
                        let result = crate::stop::check(ctx, complete_result(ctx));
                        assert_eq!(
                            result,
                            if refuse {
                                Err(AdapterError::Unsupported("native refusal".into()))
                            } else {
                                Ok(Completion::Unit(Ok(())))
                            }
                        );
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                recording.recorder.stop("returned", None).unwrap().unwrap();
                let trace = recording.trace();
                let calls = adapter.calls.load(Ordering::Acquire);
                assert_eq!(calls, if retry { 2 } else { 1 });
                drop(adapter);
                let sink = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink();
                // Every actual flush, the complete tape and one-byte torn
                // successors admit exactly their published phases.
                let mut ends = sink.marks.clone();
                ends.push(sink.bytes.len());
                for mark in sink.marks {
                    if mark < sink.bytes.len() {
                        ends.push(mark + 1);
                    }
                }
                ends.sort_unstable();
                ends.dedup();
                for end in ends {
                    let tape = hirsel_free::tape::parse(&sink.bytes[..end]).unwrap();
                    let expected = hirsel_free::projection::Projection::new(
                        if tape.end.is_some() {
                            "returned"
                        } else {
                            "truncated"
                        },
                        &tape.cut,
                        &trace,
                    );
                    for width in [1, 2, 8] {
                        let replay = super::super::replay::Replay::new(&sink.bytes[..end]).unwrap();
                        let install = replay.clone();
                        let result = Arc::new(Mutex::new(None));
                        let observed = result.clone();
                        let mut run = crate::host::BodyRun::new(width, false);
                        run.free = true;
                        unsafe { run.use_physical_dispatch() };
                        let outcome = run.run(
                            move |ctx| unsafe { install.root(ctx) },
                            move |ctx| unsafe {
                                let _role = role::Guard::new(ctx);
                                let completion = crate::stop::check(
                                    ctx,
                                    replay_request(
                                        ctx,
                                        Request::Print {
                                            stdout: 17,
                                            bytes: vec![42; 4096],
                                        },
                                        KeySet::coupled(),
                                    ),
                                );
                                *observed.lock().unwrap() = Some(completion);
                                super::super::replay::start(ctx);
                                panic!("host replay passed its terminal phase");
                            },
                        );
                        assert!(
                            matches!(outcome.ended, Err(Stop::Blocked(_))),
                            "durable={durable} retry={retry} refuse={refuse} end={end} width={width} cut={:?} {outcome:?}",
                            tape.cut
                        );
                        assert_eq!(
                            result.lock().unwrap().is_some(),
                            tape.cut
                                .get(&Vec::new())
                                .is_some_and(|at| at.phase == Phase::Done)
                        );
                        if let Some(result) = result.lock().unwrap().as_ref() {
                            assert_eq!(
                                *result,
                                if refuse {
                                    Err(AdapterError::Unsupported("native refusal".into()))
                                } else {
                                    Ok(Completion::Unit(Ok(())))
                                }
                            );
                        }
                        assert_eq!(replay.finish().unwrap(), expected);
                    }
                }
            }
        }
    }
}

unsafe extern "C" fn replay_epoch_child(ctx: *mut u8, _: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let _role = unsafe { role::Guard::new(ctx) };
    let id = unsafe { (*ctx).task_id };
    unsafe {
        let completion = crate::stop::check(
            ctx,
            replay_request(
                ctx,
                Request::Print {
                    stdout: 17,
                    bytes: vec![id as u8],
                },
                KeySet::new(vec![
                    (crate::io_keys::Key(18), Mode::Shared),
                    (
                        crate::io_keys::Key(17),
                        if id == 2 {
                            Mode::Exclusive
                        } else {
                            Mode::Shared
                        },
                    ),
                ]),
            ),
        );
        assert_eq!(completion, Ok(Completion::Unit(Ok(()))));
        super::super::replay::start(ctx);
    }
    unreachable!("child host replay past cut");
}

fn replay_epoch_tape(bytes: &[u8], expected: &hirsel_free::projection::Projection) {
    for width in [1, 2, 8] {
        let replay = super::super::replay::Replay::new(bytes).unwrap();
        let install = replay.clone();
        let mut run = crate::host::BodyRun::new(width, false);
        run.free = true;
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe { install.root(ctx) },
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                for ordinal in 0..3u32 {
                    let stack = Stack::new();
                    let child_ctx = stack.context();
                    child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
                    let path = vec![ordinal];
                    super::super::record::begin(ctx);
                    assert!(crate::stop::check(
                        ctx,
                        super::super::record::publish(ctx, Phase::Done, |_| {
                            super::super::replay::child(ctx, &path)
                        })
                    ));
                    let s = scheduler(ctx);
                    let id = s.tasks.len() as u32;
                    (*child_ctx).task_id = id;
                    (*child_ctx).task_sp =
                        stack.prepare(replay_epoch_child, child_ctx.cast(), std::ptr::null_mut());
                    let mut row = crate::tasks::Task::new(1, child_ctx, None, path, None);
                    row.stack = Some(stack);
                    s.tasks.push(Some(row));
                    crate::pool::free::schedule_saved((*ctx).run_ptr(), id, Owner::Any);
                }
                let completion = crate::stop::check(
                    ctx,
                    replay_request(
                        ctx,
                        Request::Print {
                            stdout: 17,
                            bytes: vec![0],
                        },
                        KeySet::new(vec![
                            (crate::io_keys::Key(18), Mode::Shared),
                            (crate::io_keys::Key(17), Mode::Shared),
                        ]),
                    ),
                );
                assert_eq!(completion, Ok(Completion::Unit(Ok(()))));
                super::super::replay::start(ctx);
                unreachable!("root host replay past cut");
            },
        );
        assert!(
            matches!(outcome.ended, Err(Stop::Blocked(_))),
            "{outcome:?}"
        );
        assert_eq!(&replay.finish().unwrap(), expected);
    }
}

pub(super) fn created_completion(shape: usize, word: u64) -> Completion {
    let address = crate::net::SocketAddr::of("127.0.0.1:4321".parse().unwrap());
    match shape {
        0 => Completion::Handle(Ok(word)),
        1 => Completion::ClockGrant(Ok(word)),
        2 => Completion::OptionalFile(Some(word)),
        3 => Completion::NetHandle(Ok(word)),
        4 => Completion::Listened(Ok((word, address))),
        5 => Completion::Accepted(Ok((word, address))),
        6 => Completion::Spawned(Ok(word)),
        7 => Completion::Span {
            handle: word,
            trace: [0x42; 16],
            span: [0x24; 8],
        },
        8 => Completion::Halves(Ok((word, word + 1))),
        _ => unreachable!(),
    }
}

fn created_kind(shape: usize) -> crate::io_keys::Kind {
    match shape {
        0 | 2 => crate::io_keys::Kind::File,
        8 => crate::io_keys::Kind::Half {
            stream: crate::io_keys::Key(71),
        },
        _ => crate::io_keys::Kind::Other,
    }
}

#[test]
fn native_host_created_handles_persist_creator_and_resource_without_host_words() {
    created_handles(false);
}

#[test]
fn native_discovered_host_handles_persist_registered_metadata_without_host_words() {
    created_handles(true);
}

fn created_handles(discovered: bool) {
    const WORDS: [u64; 3] = [
        0x1f23_4567_89ab_cdef,
        0x2f23_4567_89ab_cdef,
        0x3f23_4567_89ab_cdef,
    ];
    for (shape, durable) in (0..9).flat_map(|shape| [false, true].map(|durable| (shape, durable))) {
        let recording =
            super::super::record::Recording::new(b"native logical host handles", durable);
        let install = recording.clone();
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
                for (index, word) in WORDS.into_iter().enumerate() {
                    let resource = crate::io_keys::Key(if index < 2 { 17 } else { 18 });
                    let resources = [(resource, created_kind(shape)); 2];
                    let request = if shape == 8 {
                        Request::NetSplit { net: 1, stream: 2 }
                    } else {
                        Request::Stdout { io: 1 }
                    };
                    let keys = KeySet::new(vec![(crate::io_keys::ROOT, Mode::Shared)]);
                    crate::stop::check(
                        ctx,
                        if discovered {
                            prepare_with_keys(ctx, request, keys)
                        } else {
                            prepare_typed_creations(
                                ctx,
                                request,
                                keys,
                                &resources[..if shape == 8 { 2 } else { 1 }],
                            )
                        },
                    );
                    assert!(crate::stop::check(ctx, claim(ctx)));
                    if discovered {
                        crate::stop::check(
                            ctx,
                            invoke_discovered(
                                ctx,
                                |_| Ok(created_completion(shape, word)),
                                |handle| {
                                    assert!(handle == word || (shape == 8 && handle == word + 1));
                                    Some((resource, created_kind(shape)))
                                },
                            ),
                        );
                    } else {
                        crate::stop::check(ctx, invoke(ctx, |_| created_completion(shape, word)));
                    }
                    crate::stop::check(ctx, ended(ctx));
                    assert_eq!(
                        crate::stop::check(ctx, complete(ctx)),
                        created_completion(shape, word)
                    );
                    crate::stop::check(
                        ctx,
                        prepare_with_keys(
                            ctx,
                            Request::Print {
                                stdout: word,
                                bytes: vec![index as u8],
                            },
                            KeySet::new(vec![(resource, Mode::Exclusive)]),
                        ),
                    );
                    assert!(crate::stop::check(ctx, claim(ctx)));
                    crate::stop::check(ctx, invoke(ctx, |_| Completion::Unit(Ok(()))));
                    crate::stop::check(ctx, ended(ctx));
                    crate::stop::check(ctx, complete(ctx));
                }
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        let cut = recording
            .recorder
            .stop("returned", None)
            .unwrap()
            .unwrap()
            .0;
        let expected =
            hirsel_free::projection::Projection::new("returned", &cut.progress, &recording.trace());
        let bytes = Arc::try_unwrap(recording)
            .ok()
            .unwrap()
            .recorder
            .into_sink()
            .bytes;
        for word in WORDS.into_iter().flat_map(|word| [word, word + 1]) {
            assert!(
                !bytes.windows(8).any(|window| window == word.to_le_bytes()),
                "host word entered the tape"
            );
        }
        let parsed = hirsel_free::tape::parse(&bytes).unwrap();
        let records = hirsel_free::record::decode(&parsed.logs[&Vec::new()]).unwrap();
        let created: Vec<_> = records
            .iter()
            .filter_map(|record| {
                if let hirsel_free::record::Record::Completion { bytes, .. } = record {
                    Some(crate::tape_wire::decode::<Envelope>(bytes).unwrap().created)
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        assert_eq!(
            created
                .iter()
                .map(|created| (created.path.clone(), created.ordinal, created.resource))
                .collect::<Vec<_>>(),
            (0..3)
                .flat_map(|index| {
                    let count = if shape == 8 { 2 } else { 1 };
                    (0..count).map(move |field| {
                        (
                            vec![],
                            index * count + field + 1,
                            if index < 2 { 17 } else { 18 },
                        )
                    })
                })
                .collect::<Vec<_>>()
        );
        for width in [1, 2, 8] {
            let replay = super::super::replay::Replay::new(&bytes).unwrap();
            let install = replay.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe { install.root(ctx) },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    for index in 0..3 {
                        let completion = crate::stop::check(
                            ctx,
                            replay_request(
                                ctx,
                                if shape == 8 {
                                    Request::NetSplit { net: 1, stream: 2 }
                                } else {
                                    Request::Stdout { io: 1 }
                                },
                                KeySet::new(vec![(crate::io_keys::ROOT, Mode::Shared)]),
                            ),
                        );
                        let mut completion = completion;
                        let local = *handles::fields(&mut completion)
                            .unwrap()
                            .into_iter()
                            .flatten()
                            .next()
                            .unwrap();
                        assert_eq!(completion, Ok(created_completion(shape, local)));
                        assert!(!WORDS.contains(&local));
                        let resource = crate::stop::check(ctx, replay_created_key(ctx, local));
                        assert_eq!(resource.0, if index < 2 { 17 } else { 18 });
                        assert_eq!(
                            crate::stop::check(ctx, replay_created_identity(ctx, local)),
                            (resource, created_kind(shape))
                        );
                        for word in handles::fields(&mut completion)
                            .unwrap()
                            .into_iter()
                            .flatten()
                        {
                            assert_eq!(
                                crate::stop::check(ctx, replay_created_key(ctx, *word)),
                                resource
                            );
                            assert_eq!(
                                crate::stop::check(ctx, replay_created_identity(ctx, *word)),
                                (resource, created_kind(shape))
                            );
                        }
                        let completion = crate::stop::check(
                            ctx,
                            replay_request(
                                ctx,
                                Request::Print {
                                    stdout: local,
                                    bytes: vec![index as u8],
                                },
                                KeySet::new(vec![(resource, Mode::Exclusive)]),
                            ),
                        );
                        assert_eq!(completion, Ok(Completion::Unit(Ok(()))));
                    }
                    super::super::replay::start(ctx);
                    unreachable!("logical handle replay passed cut");
                },
            );
            assert!(
                matches!(outcome.ended, Err(Stop::Blocked(_))),
                "{outcome:?}"
            );
            assert_eq!(replay.finish().unwrap(), expected);
        }
    }
}

#[test]
fn missing_discovered_metadata_retains_completion_without_advancing_creators_or_ended() {
    for width in [1, 8] {
        for missing in [0, 1] {
            let recording = super::super::record::Recording::new(b"missing host metadata", false);
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
                    crate::stop::check(ctx, prepare(ctx, Request::NetSplit { net: 1, stream: 2 }));
                    assert!(crate::stop::check(ctx, claim(ctx)));
                    let result = invoke_discovered(
                        ctx,
                        |_| Ok(Completion::Halves(Ok((71, 72)))),
                        |word| {
                            (word != 71 + missing).then_some((
                                crate::io_keys::Key(17),
                                crate::io_keys::Kind::Half {
                                    stream: crate::io_keys::Key(18),
                                },
                            ))
                        },
                    );
                    assert!(matches!(result, Err(Stop::Refused(_))));
                    let pending = (*ctx).pending_host.as_ref().unwrap();
                    assert_eq!(
                        pending.completion,
                        Some(Ok(Completion::Halves(Ok((71, 72)))))
                    );
                    assert!(pending.created.is_empty());
                    assert!(pending.encoded.is_empty());
                    assert_eq!(
                        super::super::record::reserve_host_handle(ctx),
                        1,
                        "validate every field before reserving any ordinal"
                    );
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            let (cut, sweep) = recording.recorder.stop("refused", None).unwrap().unwrap();
            assert_eq!(cut.progress[&Vec::new()], Progress::new(1, Phase::Claimed));
            assert_eq!(sweep.in_doubt, [1]);
        }
    }
}
