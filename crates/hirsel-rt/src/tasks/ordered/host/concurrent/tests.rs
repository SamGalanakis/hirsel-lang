use super::*;
use crate::io_keys::{KeySet, Kind};
use crate::ordered::Policy;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicPtr, Ordering},
};

struct Checked {
    real: crate::io_real::ConcurrentIo,
    run: AtomicPtr<crate::context::RunShared>,
    tokens: Mutex<Vec<u64>>,
    finished: Mutex<Vec<u64>>,
    hide_output_count: bool,
}
impl Checked {
    fn role(&self) -> role::IdleGuard {
        // Every callback must be able to own Scheduler while the issuer's
        // head remains unresolved. Holding its role across a callback hangs.
        unsafe { role::IdleGuard::new(self.run.load(Ordering::Acquire)) }
    }
}
impl ConcurrentAdapter for Checked {
    fn keys(&self, request: &Request) -> KeySet {
        let _role = self.role();
        self.real.keys(request)
    }
    fn identity(&self, word: u64) -> Option<(crate::io_keys::Key, Kind)> {
        let _role = self.role();
        self.real.identity(word)
    }
    fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
        let _role = self.role();
        self.real.preflight(request)
    }
    fn call(&self, _: Request) -> Result<Completion, AdapterError> {
        unreachable!("owned token required")
    }
    fn call_at(&self, token: u64, request: Request) -> Result<Completion, AdapterError> {
        let _role = self.role();
        self.tokens.lock().unwrap().push(token);
        self.real.call_at(token, request)
    }
    fn call_observed_at(
        &self,
        token: u64,
        request: Request,
    ) -> (Result<Completion, AdapterError>, Option<u64>) {
        let _role = self.role();
        self.tokens.lock().unwrap().push(token);
        let (result, count) = self.real.call_observed_at(token, request);
        (result, if self.hide_output_count { None } else { count })
    }
    fn finish(&self, token: u64) {
        let _role = self.role();
        self.finished.lock().unwrap().push(token);
        self.real.finish(token);
    }
}

fn word(result: Result<Completion, AdapterError>) -> u64 {
    let Ok(Completion::Handle(Ok(word))) = result else {
        panic!("handle result")
    };
    word
}

#[test]
fn ordered_concurrent_bounded_real_files_release_role_and_publish_discovered_observations() {
    let mut reference = None;
    for width in [1, 2, 4, 8] {
        for reorder in [
            hirsel_run_config::Reorder::Off,
            hirsel_run_config::Reorder::Auto,
        ] {
            let root = tempfile::tempdir().unwrap();
            let real = crate::io_real::RealIo::grant(root.path()).unwrap();
            let adapter = Arc::new(Checked {
                real: real
                    .into_concurrent()
                    .unwrap_or_else(|_| panic!("Send adapter")),
                run: Default::default(),
                tokens: Default::default(),
                finished: Default::default(),
                hide_output_count: false,
            });
            let install = adapter.clone();
            let calls = adapter.clone();
            let output = Arc::new(Mutex::new((Vec::new(), Vec::new(), Vec::new())));
            let seen = output.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            unsafe { run.use_physical_dispatch() };
            let (outcome, trace) = run.run_traced(
                move |ctx| unsafe {
                    let mut policy = Policy::admit(&Default::default(), None);
                    policy.reorder = reorder;
                    engine::enable(ctx, policy);
                    super::super::super::events::enable(ctx);
                    super::super::super::events::initial_host(
                        ctx,
                        Some(1),
                        Some(crate::io::ENV_HANDLE),
                    );
                    install.run.store((*ctx).run_ptr(), Ordering::Release);
                },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    let directory = word(
                        request(
                            ctx,
                            &*calls,
                            Request::Directory {
                                io: 1,
                                path: String::new(),
                            },
                        )
                        .unwrap(),
                    );
                    let file = word(
                        request(
                            ctx,
                            &*calls,
                            Request::Create {
                                directory,
                                path: "data".into(),
                            },
                        )
                        .unwrap(),
                    );
                    let alias = word(
                        request(
                            ctx,
                            &*calls,
                            Request::Open {
                                directory,
                                path: "data".into(),
                            },
                        )
                        .unwrap(),
                    );
                    assert_eq!(
                        request(
                            ctx,
                            &*calls,
                            Request::FileWrite {
                                directory,
                                file,
                                bytes: vec![0, 255, 7]
                            }
                        )
                        .unwrap(),
                        Ok(Completion::Size(Ok(3)))
                    );
                    assert_eq!(
                        request(ctx, &*calls, Request::Close { handle: file }).unwrap(),
                        Ok(Completion::Unit(Ok(())))
                    );
                    assert_eq!(
                        request(ctx, &*calls, Request::Close { handle: alias }).unwrap(),
                        Ok(Completion::Unit(Ok(())))
                    );
                    assert!((*ctx).pending_ordered_host.is_none());
                    let driver = engine::driver(ctx);
                    assert!(driver.host_ops.is_empty());
                    let log = driver.events.as_ref().unwrap();
                    assert_ne!(log.host_handles[&file], log.host_handles[&alias]);
                    assert_eq!(
                        log.host_bindings[&log.host_handles[&file]].resource,
                        log.host_bindings[&log.host_handles[&alias]].resource
                    );
                    let rows: Vec<_> = log
                        .host_completions
                        .iter()
                        .map(|done| {
                            (
                                done.ordinal,
                                log.host_requests[&(done.task.clone(), done.ordinal)].clone(),
                                log.host_key_observations[&(done.task.clone(), done.ordinal)]
                                    .clone(),
                                done.observation.clone().expect("complete result discovery"),
                            )
                        })
                        .collect();
                    assert_eq!(rows.len(), 6);
                    assert_eq!(
                        rows[0].1.as_ref().unwrap().authorities,
                        [super::super::super::events::HostAuthority::RootIo]
                    );
                    assert!(rows.iter().skip(1).all(|row| row.1.is_some()));
                    assert!(rows.iter().all(|row| row.2.is_some()));
                    let canonical = log.canonical().expect("complete bounded host trace");
                    assert_eq!(canonical.len(), 32);
                    for ordinal in 0..6 {
                        let events: Vec<_> = canonical
                            .iter()
                            .filter(|event| event.ordinal == ordinal)
                            .collect();
                        assert!(matches!(
                            events[0].outcome,
                            super::super::super::events::CanonicalOutcome::HostRequest(_)
                        ));
                        assert!(matches!(
                            events.last().unwrap().outcome,
                            super::super::super::events::CanonicalOutcome::HostFinished
                        ));
                        assert_eq!(
                            events
                                .iter()
                                .map(|event| event.subevent)
                                .collect::<Vec<_>>(),
                            (0..events.len() as u32).collect::<Vec<_>>()
                        );
                    }
                    let jsonl = log.jsonl().expect("canonical trace serializes");
                    let parsed: Vec<serde_json::Value> = std::str::from_utf8(&jsonl)
                        .unwrap()
                        .lines()
                        .map(|line| serde_json::from_str(line).unwrap())
                        .collect();
                    assert_eq!(parsed.len(), canonical.len());
                    assert!(
                        parsed
                            .iter()
                            .all(|row| row["schema"] == "hirsel.ordered.events@1")
                    );
                    *seen.lock().unwrap() = (rows, canonical.to_vec(), jsonl);
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            let trace = trace.expect("complete host trace includes final root end");
            let destination = tempfile::tempdir().unwrap();
            let path = destination.path().join("events.jsonl");
            trace
                .publish(
                    crate::host_paths::RecorderDestination::pin(&path).unwrap(),
                    &[],
                    &[],
                )
                .unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), trace.bytes());
            let last: serde_json::Value = serde_json::from_str(
                std::str::from_utf8(trace.bytes())
                    .unwrap()
                    .lines()
                    .last()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(last["outcome"]["kind"], "root_end");
            assert_eq!(
                std::fs::read(root.path().join("data")).unwrap(),
                [0, 255, 7]
            );
            assert_eq!(*adapter.tokens.lock().unwrap(), [1, 2, 3, 4, 5, 6]);
            assert_eq!(*adapter.finished.lock().unwrap(), [1, 2, 3, 4, 5, 6]);
            let rows = (output.lock().unwrap().clone(), trace.bytes().to_vec());
            if let Some(reference) = &reference {
                assert_eq!(&rows, reference);
            } else {
                reference = Some(rows);
            }
        }
    }
}

#[test]
fn ordered_concurrent_bounded_cancellation_preserves_checks_and_rejects_waits_before_issue() {
    for width in [1, 8] {
        let root = tempfile::tempdir().unwrap();
        let real = crate::io_real::RealIo::grant(root.path()).unwrap();
        let adapter = Arc::new(Checked {
            real: real
                .into_concurrent()
                .unwrap_or_else(|_| panic!("Send adapter")),
            run: Default::default(),
            tokens: Default::default(),
            finished: Default::default(),
            hide_output_count: false,
        });
        let install = adapter.clone();
        let calls = adapter.clone();
        let mut run = crate::host::BodyRun::new(width, true);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            move |ctx| unsafe {
                engine::enable(ctx, Policy::admit(&Default::default(), None));
                super::super::super::events::enable(ctx);
                install.run.store((*ctx).run_ptr(), Ordering::Release);
            },
            move |ctx| unsafe {
                let _role = role::Guard::new(ctx);
                assert!(matches!(
                    request(
                        ctx,
                        &*calls,
                        Request::SleepUntil {
                            clock: 99,
                            deadline: 1
                        }
                    ),
                    Err(Stop::Refused(_))
                ));
                assert!((*ctx).pending_ordered_host.is_none());
                assert!(engine::driver(ctx).host_ops.is_empty());
                assert_eq!(engine::driver(ctx).next_host_token, 1);
                let directory = word(
                    request(
                        ctx,
                        &*calls,
                        Request::Directory {
                            io: 1,
                            path: String::new(),
                        },
                    )
                    .unwrap(),
                );
                let file = word(
                    request(
                        ctx,
                        &*calls,
                        Request::Create {
                            directory,
                            path: "data".into(),
                        },
                    )
                    .unwrap(),
                );
                scheduler(ctx).cancel(0, CANCEL_REQUESTED);
                assert_eq!(
                    request(
                        ctx,
                        &*calls,
                        Request::FileWrite {
                            directory,
                            file,
                            bytes: vec![0, 255, 7]
                        }
                    )
                    .unwrap(),
                    Ok(Completion::Size(Err(crate::io::IoError::Canceled(
                        CANCEL_REQUESTED
                    ))))
                );
                // Argument/authority validation still takes precedence over a
                // cancellation, and a rejected operation issues no write.
                assert_eq!(
                    request(
                        ctx,
                        &*calls,
                        Request::FileWrite {
                            directory,
                            file: 9999,
                            bytes: vec![1]
                        }
                    )
                    .unwrap(),
                    Err(AdapterError::InvalidHandle(9999))
                );
                assert_eq!(
                    request(ctx, &*calls, Request::Close { handle: file }).unwrap(),
                    Ok(Completion::Unit(Ok(())))
                );
                assert!((*ctx).pending_ordered_host.is_none());
                let log = engine::driver(ctx).events.as_ref().unwrap();
                assert_eq!(log.host_completions.len(), 5);
                assert_eq!(log.host_initials.len(), 5);
                assert_eq!(log.host_initials[&(vec![], 2)], log.host_completions[2]);
                assert!(log.host_completions[2].observation.is_some());
                assert!(
                    log.host_completions[3].observation.is_none(),
                    "unknown refusal authority must not leak its word"
                );
                STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert!(std::fs::read(root.path().join("data")).unwrap().is_empty());
        assert_eq!(*adapter.tokens.lock().unwrap(), [1, 2, 5]);
        assert_eq!(*adapter.finished.lock().unwrap(), [1, 2, 3, 4, 5]);
    }
}

struct Output {
    bytes: Vec<u8>,
    mode: u8,
}
impl std::io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = if self.mode == 1 {
            let left = 5usize.saturating_sub(self.bytes.len());
            if left == 0 {
                return Err(std::io::Error::other("partial output"));
            }
            left.min(bytes.len())
        } else {
            bytes.len()
        };
        self.bytes.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.mode == 2 {
            Err(std::io::Error::other("flush failed"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn ordered_concurrent_output_canonical_traces_keep_actual_prefixes_and_refuse_unknown_counts() {
    use super::super::super::events::{CanonicalItem, CanonicalOutcome};
    for mode in 0..5 {
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let storage = Arc::new(Mutex::new(Output {
                    bytes: vec![],
                    mode,
                }));
                let mut real = crate::io_real::RealIo::grant(&std::env::temp_dir()).unwrap();
                real.set_send_output(crate::host::SendSink::shared(storage.clone()));
                real.set_send_stderr(crate::host::SendSink::shared(storage.clone()));
                let adapter = Arc::new(Checked {
                    real: real
                        .into_concurrent()
                        .unwrap_or_else(|_| panic!("Send output")),
                    run: Default::default(),
                    tokens: Default::default(),
                    finished: Default::default(),
                    hide_output_count: mode == 4,
                });
                let install = adapter.clone();
                let calls = adapter.clone();
                let output = Arc::new(Mutex::new(None));
                let seen = output.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        let mut policy = Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        engine::enable(ctx, policy);
                        super::super::super::events::enable(ctx);
                        super::super::super::events::initial_host(ctx, Some(1), None);
                        install.run.store((*ctx).run_ptr(), Ordering::Release);
                    },
                    move |ctx| unsafe {
                        let _role = role::Guard::new(ctx);
                        let stdout =
                            word(request(ctx, &*calls, Request::Stdout { io: 1 }).unwrap());
                        let stderr =
                            word(request(ctx, &*calls, Request::Stderr { io: 1 }).unwrap());
                        assert_ne!(stdout, stderr);
                        if mode == 3 {
                            scheduler(ctx).cancel(0, 7);
                        }
                        for request_value in [
                            Request::Print {
                                stdout,
                                bytes: b"stdout\0\xff".to_vec(),
                            },
                            Request::PrintStderr {
                                stderr,
                                bytes: b"stderr\n".to_vec(),
                            },
                        ] {
                            let expected = match mode {
                                1 | 2 => Err(crate::io::IoError::Fault(1)),
                                3 => Err(crate::io::IoError::Canceled(7)),
                                _ => Ok(()),
                            };
                            assert_eq!(
                                request(ctx, &*calls, request_value).unwrap(),
                                Ok(Completion::Unit(expected))
                            );
                        }
                        assert!((*ctx).pending_ordered_host.is_none());
                        let log = engine::driver(ctx).events.as_ref().unwrap();
                        if mode == 4 {
                            assert!(log.canonical().is_err());
                            assert!(log.jsonl().is_err());
                            assert!(!log.canonical.iter().any(|event| matches!(
                                event.outcome,
                                CanonicalOutcome::HostOutput(_)
                            )));
                        } else {
                            let events = log.canonical().unwrap();
                            let outputs: Vec<_> = events
                                .iter()
                                .filter_map(|event| {
                                    if let CanonicalOutcome::HostOutput(bytes) = &event.outcome {
                                        Some((event, bytes))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            assert_eq!(outputs.len(), 2);
                            assert!(matches!(outputs[0].0.item, CanonicalItem::HostResource(_)));
                            assert_eq!(
                                outputs[0].0.item, outputs[1].0.item,
                                "aliased logical sink"
                            );
                            assert_eq!(outputs[1].0.version, outputs[0].0.version + 2);
                            let actual: Vec<_> = outputs
                                .iter()
                                .flat_map(|(_, bytes)| bytes.iter().copied())
                                .collect();
                            let expected = match mode {
                                1 => &b"stdou"[..],
                                3 => &b""[..],
                                _ => &b"stdout\0\xffstderr\n"[..],
                            };
                            assert_eq!(actual, expected);
                            *seen.lock().unwrap() = Some(log.jsonl().unwrap());
                        }
                        STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let expected = match mode {
                    1 => &b"stdou"[..],
                    3 => &b""[..],
                    _ => &b"stdout\0\xffstderr\n"[..],
                };
                assert_eq!(storage.lock().unwrap().bytes, expected);
                let serialized = output.lock().unwrap().clone();
                if let Some(reference) = &reference {
                    assert_eq!(&serialized, reference);
                } else {
                    reference = Some(serialized);
                }
                assert_eq!(*adapter.finished.lock().unwrap(), [1, 2, 3, 4]);
            }
        }
    }
}
