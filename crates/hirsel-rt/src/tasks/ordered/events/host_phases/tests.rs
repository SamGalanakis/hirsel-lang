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
