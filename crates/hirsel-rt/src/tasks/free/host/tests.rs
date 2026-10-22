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
