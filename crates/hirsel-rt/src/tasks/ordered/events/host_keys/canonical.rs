use super::super::{Boundary, CanonicalItem, CanonicalOutcome, HostCompletion, Log, Transition};
use super::*;
use crate::io::{Completion, Request};
use crate::ordered::footprint::ObjectId;

fn pending() -> Log {
    let mut log = Log::default();
    let key = (vec![1], 0);
    log.host_entries.insert(
        key.clone(),
        Boundary {
            task: vec![1],
            ordinal: 0,
            op: Some("host.request"),
            transition: Transition::Keep,
            retirement: 3,
            before: crate::budget::stretch::Stretch::new(crate::budget::stretch::Budgets {
                fuel: false,
                memory: false,
            }),
            credit: 0,
            native_outcome: None,
        },
    );
    let id = HostHandleId {
        operation: ObjectId {
            creator: vec![],
            ordinal: 77,
        },
        field: 0,
    };
    let request = super::super::host_requests::project(
        &Request::Print {
            stdout: 99,
            bytes: vec![0, 255, 7],
        },
        |_| Some(super::super::HostAuthority::Created(id.clone())),
    )
    .unwrap();
    log.host_requests.insert(key.clone(), Some(request));
    log.host_key_observations.insert(
        key.clone(),
        Some(vec![(Resource::Coupled, Mode::Exclusive)]),
    );
    log.host_creations.insert(key, vec![]);
    let result = Ok(Completion::Unit(Ok(())));
    let observation = super::super::host_values::project(&result, |_| None).unwrap();
    log.host_completions.push(HostCompletion {
        task: vec![1],
        ordinal: 0,
        op: "host.request",
        result,
        observation: Some(observation),
    });
    log
}

#[test]
fn canonical_host_requires_every_owned_observation_before_emitting_any_subevent() {
    for missing in 0..11 {
        let mut log = pending();
        let key = (vec![1], 0);
        match missing {
            0 => {
                log.host_requests.remove(&key);
            }
            1 => {
                log.host_requests.insert(key.clone(), None);
            }
            2 => {
                log.host_key_observations.remove(&key);
            }
            3 => {
                log.host_key_observations.insert(key.clone(), None);
            }
            4 => {
                log.host_creations.remove(&key);
            }
            5 => log.host_completions.clear(),
            6 => log.host_completions[0].observation = None,
            7 => log.host_completions[0].ordinal = 1,
            8 => {
                log.host_polled.insert(key.clone());
            }
            9 => log.host_entries.get_mut(&key).unwrap().transition = Transition::Park,
            10 => log.host_completions[0]
                .observation
                .as_mut()
                .unwrap()
                .handles
                .push(HostHandleId {
                    operation: ObjectId {
                        creator: vec![],
                        ordinal: 99,
                    },
                    field: 0,
                }),
            _ => unreachable!(),
        }
        assert!(log.certify_host(&[1], 0).is_err());
        assert!(
            log.canonical.is_empty(),
            "failed evidence installed partial subevents"
        );
        assert!(log.versions.is_empty());
        assert!(log.host_certified.is_empty());
        assert!(log.canonical().is_err());
    }
    let mut log = pending();
    log.certify_host(&[1], 0).unwrap();
    assert!(
        log.canonical().is_err(),
        "completion evidence cannot replace its post-effect continuation"
    );
    assert_eq!(log.canonical.len(), 3);
    assert!(matches!(
        log.canonical[0].outcome,
        CanonicalOutcome::HostRequest(_)
    ));
    let CanonicalOutcome::HostAccess { mode, result } = &log.canonical[1].outcome else {
        panic!("key outcome")
    };
    assert_eq!(*mode, Mode::Exclusive);
    assert_eq!(
        crate::tape_wire::decode::<Result<Completion, crate::io::AdapterError>>(&result.payload)
            .unwrap(),
        Ok(Completion::Unit(Ok(())))
    );
    assert!(
        log.certify_host(&[1], 0).is_err(),
        "one completion cannot emit twice"
    );
    assert_eq!(log.canonical.len(), 3);
}

#[test]
fn canonical_host_alias_observations_keep_the_original_handle_creation_version() {
    let mut log = pending();
    let handle = HostHandleId {
        operation: ObjectId {
            creator: vec![1],
            ordinal: 0,
        },
        field: 0,
    };
    let row = Creation {
        handle: handle.clone(),
        resource: Resource::Created(handle.clone()),
        kind: ResourceKind::Other,
    };
    log.host_completions[0].result = Ok(Completion::Handle(Ok(99)));
    log.host_completions[0].observation = Some(
        super::super::host_values::project(&log.host_completions[0].result, |_| {
            Some(handle.clone())
        })
        .unwrap(),
    );
    log.host_creations.insert((vec![1], 0), vec![row.clone()]);
    log.certify_host(&[1], 0).unwrap();
    assert_eq!(log.versions[&CanonicalItem::HostHandle(handle.clone())], 1);
    let key = (vec![1], 1);
    let mut boundary = pending().host_entries.remove(&(vec![1], 0)).unwrap();
    boundary.ordinal = 1;
    log.host_entries.insert(key.clone(), boundary);
    let request = log.host_requests[&(vec![1], 0)].clone();
    log.host_requests.insert(key.clone(), request);
    log.host_key_observations.insert(key.clone(), Some(vec![]));
    log.host_creations.insert(key, vec![row]);
    let mut completion = log.host_completions[0].clone();
    completion.ordinal = 1;
    log.host_completions.push(completion);
    log.certify_host(&[1], 1).unwrap();
    let aliases: Vec<_> = log
        .canonical
        .iter()
        .filter(|event| event.item == CanonicalItem::HostHandle(handle.clone()))
        .collect();
    assert_eq!(aliases.len(), 2);
    assert_eq!(aliases[0].version, 1);
    assert_eq!(aliases[1].version, 2);
    assert!(matches!(
        aliases[0].outcome,
        CanonicalOutcome::HostCreated(_)
    ));
    assert!(matches!(aliases[1].outcome, CanonicalOutcome::HostAlias(_)));
    assert_eq!(
        log.canonical
            .iter()
            .filter(|event| matches!(event.outcome, CanonicalOutcome::HostResourceCreated))
            .count(),
        1
    );
}
