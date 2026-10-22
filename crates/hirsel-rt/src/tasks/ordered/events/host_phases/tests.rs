use super::*;
use crate::io::{Completion, WaitError};

fn empty() -> Stretch {
    Stretch::new(crate::budget::stretch::Budgets {
        fuel: false,
        memory: false,
    })
}
fn pending(result: Result<Completion, AdapterError>) -> Log {
    let mut log = Log::default();
    let key = (vec![1], 0);
    let request = host_requests::project(
        &Request::SleepUntil {
            clock: 99,
            deadline: 100,
        },
        |_| {
            Some(HostAuthority::Created(HostHandleId {
                operation: ObjectId {
                    creator: vec![],
                    ordinal: 7,
                },
                field: 0,
            }))
        },
    )
    .unwrap();
    log.home_requests.insert(key.clone());
    log.host_requests.insert(key.clone(), Some(request));
    log.host_key_observations.insert(
        key.clone(),
        Some(vec![(host_keys::Resource::Coupled, Mode::Exclusive)]),
    );
    log.host_entries.insert(
        key.clone(),
        Boundary {
            task: key.0.clone(),
            ordinal: 0,
            op: Some("host.request"),
            transition: Transition::Keep,
            retirement: 1,
            before: empty(),
            credit: 0,
            native_outcome: None,
        },
    );
    let observation = host_values::project(&result, |_| None).ok();
    log.host_initials.insert(
        key.clone(),
        HostCompletion {
            task: key.0,
            ordinal: 0,
            op: "host.request",
            result,
            observation,
        },
    );
    log
}
fn continuation(log: &mut Log, next: Next) {
    let key = (vec![1], 0);
    let events = log.continue_home(&key, &next).unwrap();
    let transition = match next {
        Next::Yield => Transition::Yield,
        Next::Park => Transition::Park,
        Next::Keep => Transition::Keep,
        _ => unreachable!(),
    };
    log.subevents(
        &Boundary {
            task: key.0,
            ordinal: 0,
            op: None,
            transition,
            retirement: 2,
            before: empty(),
            credit: 0,
            native_outcome: None,
        },
        "host.request",
        events,
    );
}
fn attempt(result: Result<Completion, AdapterError>, sequence: u64) -> HostPollCompletion {
    HostPollCompletion {
        completion: HostCompletion {
            task: vec![1],
            ordinal: 0,
            op: "host.request",
            observation: host_values::project(&result, |_| None).ok(),
            result,
        },
        retirement: 3,
        idle: true,
        sequence,
    }
}

#[test]
fn home_phases_require_initial_request_authority_keys_and_scalar_result() {
    let key = (vec![1], 0);
    for missing in 0..6 {
        let mut log = pending(Err(AdapterError::Blocked));
        match missing {
            0 => {
                log.host_requests.insert(key.clone(), None);
            }
            1 => {
                log.host_key_observations.remove(&key);
            }
            2 => {
                log.host_initials.remove(&key);
            }
            3 => {
                log.host_initials.get_mut(&key).unwrap().observation = None;
            }
            4 => {
                log.home_requests.clear();
            }
            5 => {
                log.host_key_observations.insert(key.clone(), Some(vec![]));
            }
            _ => unreachable!(),
        }
        log.start_home(&[1], 0);
        assert!(log.canonical.is_empty());
        assert!(log.versions.is_empty());
        assert!(log.canonical().is_err());
    }
}

#[test]
fn home_phases_preserve_one_ordinal_and_publish_each_attempt_before_owner_completion() {
    for result in [
        Ok(Completion::Wait(Ok(()))),
        Ok(Completion::Wait(Err(WaitError::Canceled(5)))),
    ] {
        let mut log = pending(Err(AdapterError::Blocked));
        log.start_home(&[1], 0);
        assert!(log.canonical().is_err());
        continuation(&mut log, Next::Yield);
        continuation(&mut log, Next::Park);
        log.attempt_home(
            &(vec![1], 0),
            &attempt(Err(AdapterError::Blocked), 0),
            empty(),
        );
        let done = attempt(result, 1);
        log.attempt_home(&(vec![1], 0), &done, empty());
        assert!(
            log.canonical().is_err(),
            "completed poll still needs owner return"
        );
        log.return_home(&(vec![1], 0), &done.completion, 4, empty());
        let events = log.canonical().unwrap();
        assert_eq!(events.len(), 8);
        assert!(
            events
                .iter()
                .all(|event| event.task == [1] && event.ordinal == 0)
        );
        assert_eq!(
            events
                .iter()
                .map(|event| event.subevent)
                .collect::<Vec<_>>(),
            (0..8).collect::<Vec<_>>()
        );
        let keys: Vec<_> = events
            .iter()
            .filter(|event| matches!(event.item, CanonicalItem::HostResource(_)))
            .collect();
        assert_eq!(
            keys.iter().map(|event| event.version).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(
            keys.iter()
                .map(|event| event.retirement)
                .collect::<Vec<_>>(),
            [1, 3, 3]
        );
        assert!(matches!(events[6].outcome, CanonicalOutcome::HostResult(_)));
        assert!(matches!(events[7].outcome, CanonicalOutcome::HostFinished));
        assert!(log.jsonl().is_ok());
    }
}

#[test]
fn home_phases_never_certify_missing_or_inconsistent_attempts() {
    for missing in 0..4 {
        let mut log = pending(Err(AdapterError::Blocked));
        log.start_home(&[1], 0);
        continuation(&mut log, Next::Park);
        let done = attempt(Ok(Completion::Wait(Ok(()))), 1);
        match missing {
            0 => {} // Completion without any completing attempt.
            1 => {
                let mut bad = done.clone();
                bad.completion.observation = None;
                log.attempt_home(&(vec![1], 0), &bad, empty());
                log.attempt_home(&(vec![1], 0), &done, empty());
            }
            2 => {
                log.attempt_home(&(vec![1], 0), &done, empty());
            }
            3 => {
                let mut bad = done.clone();
                bad.completion
                    .observation
                    .as_mut()
                    .unwrap()
                    .handles
                    .push(HostHandleId {
                        operation: ObjectId {
                            creator: vec![1],
                            ordinal: 8,
                        },
                        field: 0,
                    });
                log.attempt_home(&(vec![1], 0), &bad, empty());
                log.attempt_home(&(vec![1], 0), &done, empty());
            }
            _ => unreachable!(),
        }
        let mut returned = done.completion;
        if missing == 2 {
            returned.observation = attempt(Ok(Completion::Wait(Err(WaitError::Canceled(7)))), 2)
                .completion
                .observation;
        }
        log.return_home(&(vec![1], 0), &returned, 4, empty());
        assert!(log.canonical().is_err());
        assert!(log.jsonl().is_err());
    }
}

#[test]
fn immediate_home_scalar_needs_its_post_effect_continuation() {
    let mut log = pending(Ok(Completion::Wait(Ok(()))));
    log.start_home(&[1], 0);
    assert!(log.canonical().is_err());
    let done = log.host_initials[&(vec![1], 0)].clone();
    log.return_home(&(vec![1], 0), &done, 1, empty());
    assert!(log.canonical().is_err());
    continuation(&mut log, Next::Yield);
    assert_eq!(log.canonical().unwrap().len(), 4);
}

#[test]
fn home_wait_identity_requires_a_saved_valid_phase_without_completion() {
    let mut log = pending(Err(AdapterError::Blocked));
    log.start_home(&[1], 0);
    assert!(log.home_wait(&[1]).is_none());
    continuation(&mut log, Next::Park);
    assert_eq!(
        log.home_wait(&[1]),
        Some((
            "clock.sleep_until",
            ObjectId {
                creator: vec![1],
                ordinal: 0
            }
        ))
    );
    assert!(log.home_wait(&[2]).is_none());
    log.home_phases.get_mut(&(vec![1], 0)).unwrap().valid = false;
    assert!(log.home_wait(&[1]).is_none());
    log.home_phases.get_mut(&(vec![1], 0)).unwrap().valid = true;
    let done = attempt(Ok(Completion::Wait(Ok(()))), 0);
    log.attempt_home(&(vec![1], 0), &done, empty());
    assert!(
        log.home_wait(&[1]).is_none(),
        "completed effect cannot be certified as a blocked wait"
    );
}

#[test]
fn ordered_host_terminal_phase_validation_preserves_missing_and_conflicting_evidence() {
    let key = (vec![1], 0);
    for corrupt in 0..5 {
        let mut log = pending(Err(AdapterError::Blocked));
        log.start_home(&[1], 0);
        continuation(&mut log, Next::Park);
        match corrupt {
            0 => {
                log.home_phases.get_mut(&key).unwrap().valid = false;
            }
            1 => {
                log.host_initials.remove(&key);
            }
            2 => {
                log.host_initials.get_mut(&key).unwrap().observation = None;
            }
            3 => {
                log.invalid_home.insert(key.clone());
            }
            4 => {
                log.host_certified.insert((vec![9], 0));
            }
            _ => unreachable!(),
        }
        let before = log.canonical.clone();
        let phases = log.home_phases.len();
        assert!(log.terminal_hosts().is_err());
        assert_eq!(log.canonical, before);
        assert_eq!(log.home_phases.len(), phases);
        assert!(log.canonical().is_err());
    }
    for done in [false, true] {
        let mut log = pending(if done {
            Ok(Completion::Wait(Ok(())))
        } else {
            Err(AdapterError::Blocked)
        });
        log.start_home(&[1], 0);
        if !done {
            continuation(&mut log, Next::Park);
        }
        assert_eq!(
            log.terminal_hosts().unwrap(),
            [(
                CanonicalItem::HostOperation(ObjectId {
                    creator: vec![1],
                    ordinal: 0
                }),
                CanonicalOutcome::HostAbandoned { completed: done },
            )]
        );
        assert!(
            !log.canonical
                .iter()
                .any(|event| matches!(event.outcome, CanonicalOutcome::HostFinished))
        );
        assert!(log.home_phases.is_empty());
    }
}

fn grant(log: &mut Log, ordinal: u64, request: Request, result: Result<Completion, AdapterError>) {
    let mut fixture = pending(Err(AdapterError::Blocked));
    let old = (vec![1], 0);
    let key = (vec![1], ordinal);
    let mut entry = fixture.host_entries.remove(&old).unwrap();
    entry.ordinal = ordinal;
    log.host_entries.insert(key.clone(), entry);
    log.home_requests.insert(key.clone());
    let request = host_requests::project(&request, |word| {
        if word == 1 {
            Some(HostAuthority::RootIo)
        } else {
            log.host_handles
                .get(&word)
                .cloned()
                .map(HostAuthority::Created)
        }
    })
    .unwrap();
    log.host_requests.insert(key.clone(), Some(request));
    log.host_key_observations.insert(
        key.clone(),
        Some(vec![(host_keys::Resource::Coupled, Mode::Exclusive)]),
    );
    log.discover_home(&key, &result).unwrap();
    let observation =
        host_values::project(&result, |word| log.host_handles.get(&word).cloned()).ok();
    log.host_initials.insert(
        key,
        HostCompletion {
            task: vec![1],
            ordinal,
            op: "host.request",
            result,
            observation,
        },
    );
    log.start_home(&[1], ordinal);
}
fn finish_grant(log: &mut Log, ordinal: u64) {
    let key = (vec![1], ordinal);
    let done = log.host_initials[&key].clone();
    log.return_home(&key, &done, 1, empty());
    let events = log.continue_home(&key, &Next::Yield).unwrap();
    log.subevents(
        &Boundary {
            task: vec![1],
            ordinal,
            op: None,
            transition: Transition::Yield,
            retirement: 1,
            before: empty(),
            credit: 0,
            native_outcome: None,
        },
        "host.request",
        events,
    );
}

#[test]
fn home_grants_preserve_repeated_aliases_and_split_field_parent_metadata() {
    let mut log = Log::default();
    grant(
        &mut log,
        0,
        Request::MonotonicClock { io: 1 },
        Ok(Completion::ClockGrant(Ok(88))),
    );
    assert!(
        log.canonical().is_err(),
        "creation needs its owner continuation"
    );
    finish_grant(&mut log, 0);
    let parent = log.host_handles[&88].clone();
    assert_eq!(parent.operation.ordinal, 0);
    grant(
        &mut log,
        1,
        Request::MonotonicClock { io: 1 },
        Ok(Completion::ClockGrant(Ok(88))),
    );
    finish_grant(&mut log, 1);
    assert_eq!(log.host_handles[&88], parent);
    assert_eq!(log.versions[&CanonicalItem::HostHandle(parent)], 2);
    for ordinal in [2, 3] {
        grant(
            &mut log,
            ordinal,
            Request::NetSplit {
                net: 88,
                stream: 88,
            },
            Ok(Completion::Halves(Ok((90, 91)))),
        );
        assert!(log.canonical().is_err());
        finish_grant(&mut log, ordinal);
        for (field, word) in [(0, 90), (1, 91)] {
            let handle = &log.host_handles[&word];
            assert_eq!((handle.operation.ordinal, handle.field), (2, field));
            assert_eq!(
                log.host_bindings[handle].resource,
                host_keys::Resource::Coupled
            );
            assert_eq!(
                log.host_bindings[handle].kind,
                host_keys::ResourceKind::Half {
                    stream: host_keys::Resource::Coupled
                }
            );
            assert_eq!(
                log.versions[&CanonicalItem::HostHandle(handle.clone())],
                ordinal - 1
            );
        }
    }
    let events = log.canonical().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.outcome, CanonicalOutcome::HostCreated(_)))
            .count(),
        3
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.outcome, CanonicalOutcome::HostAlias(_)))
            .count(),
        3
    );
}

#[test]
fn home_discovery_errors_permanently_refuse_export_despite_stale_rows_or_known_handles() {
    for cause in 0..4 {
        let mut log = pending(Err(AdapterError::Blocked));
        let key = (vec![1], 0);
        let result = Ok(Completion::ClockGrant(Ok(88)));
        match cause {
            0 | 1 => {
                let key = if cause == 0 {
                    crate::io_keys::Key(17)
                } else {
                    crate::io_keys::COUPLED
                };
                let kind = if cause == 0 {
                    crate::io_keys::Kind::Other
                } else {
                    crate::io_keys::Kind::Sink { file_backed: false }
                };
                let rows = log
                    .host_created(
                        ObjectId {
                            creator: vec![2],
                            ordinal: 7,
                        },
                        &result,
                        &BTreeMap::from([(88, (key, kind))]),
                    )
                    .unwrap();
                // An old creation row cannot rescue a conflicting resource/kind.
                log.host_creations.insert((vec![1], 0), rows);
            }
            2 => {
                log.host_requests.insert(key.clone(), None);
            }
            3 => {}
            _ => unreachable!(),
        }
        let invalid = if cause == 3 {
            Ok(Completion::Halves(Ok((88, 88))))
        } else {
            result.clone()
        };
        assert!(log.discover_home(&key, &invalid).is_err());
        assert!(log.invalid_home.contains(&key));
        assert!(!log.host_creations.contains_key(&key));
        let observation =
            host_values::project(&result, |word| log.host_handles.get(&word).cloned()).ok();
        log.host_initials.get_mut(&key).unwrap().observation = observation;
        log.start_home(&[1], 0);
        assert!(log.canonical().is_err());
        // Even synthetic terminal closure cannot bypass invalid discovery.
        log.closed_blocked = true;
        log.host_entries.clear();
        assert!(log.jsonl().is_err());
    }
}

#[test]
fn home_split_rejects_missing_or_conflicting_parent_resource_before_installing_halves() {
    for known in [false, true] {
        let mut log = pending(Err(AdapterError::Blocked));
        let parent = HostHandleId {
            operation: ObjectId {
                creator: vec![2],
                ordinal: 7,
            },
            field: 0,
        };
        if known {
            log.host_bindings.insert(
                parent.clone(),
                host_keys::Creation {
                    handle: parent.clone(),
                    resource: host_keys::Resource::Created(parent.clone()),
                    kind: host_keys::ResourceKind::Other,
                },
            );
        }
        let request = host_requests::project(
            &Request::NetSplit {
                net: 99,
                stream: 99,
            },
            |_| Some(HostAuthority::Created(parent.clone())),
        )
        .unwrap();
        let key = (vec![1], 0);
        log.host_requests.insert(key.clone(), Some(request));
        assert!(
            log.discover_home(&key, &Ok(Completion::Halves(Ok((90, 91)))))
                .is_err()
        );
        assert!(!log.host_handles.contains_key(&90));
        assert!(!log.host_handles.contains_key(&91));
        assert!(log.invalid_home.contains(&key));
        assert!(log.jsonl().is_err());
    }
}
