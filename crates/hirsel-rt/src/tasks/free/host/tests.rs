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
                    crate::stop::check(ctx, invoke(ctx, |_| Completion::Unit(Ok(()))));
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
        }
    }
}
