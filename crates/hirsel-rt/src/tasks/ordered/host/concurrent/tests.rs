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
            });
            let install = adapter.clone();
            let calls = adapter.clone();
            let output = Arc::new(Mutex::new((Vec::new(), Vec::new(), Vec::new())));
            let seen = output.clone();
            let mut run = crate::host::BodyRun::new(width, true);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
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
            assert_eq!(
                std::fs::read(root.path().join("data")).unwrap(),
                [0, 255, 7]
            );
            assert_eq!(*adapter.tokens.lock().unwrap(), [1, 2, 3, 4, 5, 6]);
            assert_eq!(*adapter.finished.lock().unwrap(), [1, 2, 3, 4, 5, 6]);
            let rows = output.lock().unwrap().clone();
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
