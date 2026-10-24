//! Initialized request projection. The field walk follows Request's variants
//! and the existing tape codec, including handles nested in process arguments.
use super::host_values::HostHandleId;
use crate::io::Request;

/// Initial authorities precede operations; never synthesize creator ordinals
/// for them. Owner approval of proposal614011, design4.2.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(in crate::tasks::ordered) enum HostAuthority {
    RootIo,
    RootEnv,
    RootArgs,
    Created(HostHandleId),
}
impl From<HostHandleId> for HostAuthority {
    fn from(handle: HostHandleId) -> Self {
        Self::Created(handle)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tasks::ordered) struct HostRequestValue {
    pub payload: Vec<u8>,
    pub authorities: Vec<HostAuthority>,
}

pub(super) fn project(
    request: &Request,
    identity: impl FnMut(u64) -> Option<HostAuthority>,
) -> Result<HostRequestValue, &'static str> {
    let (payload, authorities) = crate::io_request_observation::project(request, identity)?;
    Ok(HostRequestValue {
        payload,
        authorities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_initial_authorities_are_explicit_and_never_invent_creator_ordinals() {
        use crate::ordered::Policy;
        use crate::tasks::ordered::{engine, events, host};
        for width in [1, 8] {
            let io = 0x1234_5678 + u64::from(width);
            let env = io + 1;
            let mut run = crate::host::BodyRun::new(width, true);
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    engine::enable(ctx, Policy::admit(&Default::default(), None));
                    events::enable(ctx);
                    events::initial_host(ctx, Some(io), Some(env));
                    let log = engine::driver(ctx).events.as_ref().unwrap();
                    assert!(log.ordinals.is_empty());
                    assert!(log.host_handles.is_empty());
                    assert!(log.host_bindings.is_empty());
                    assert!(log.canonical.is_empty());
                },
                move |ctx| unsafe {
                    for (request, expected) in [
                        (Request::Stdout { io }, Some(HostAuthority::RootIo)),
                        (
                            Request::EnvGet {
                                env,
                                name: "NAME".into(),
                            },
                            Some(HostAuthority::RootEnv),
                        ),
                        (Request::Stdout { io: 1 }, None),
                        (
                            Request::EnvGet {
                                env: crate::io::ENV_HANDLE,
                                name: "NAME".into(),
                            },
                            None,
                        ),
                    ] {
                        host::begin(ctx, false).unwrap();
                        events::request_host(ctx, &request);
                        let log = engine::driver(ctx).events.as_ref().unwrap();
                        let ordinal = log.ordinals[&Vec::new()] - 1;
                        let projected = &log.host_requests[&(Vec::new(), ordinal)];
                        if let Some(expected) = expected {
                            let projected = projected.as_ref().unwrap();
                            assert_eq!(projected.authorities, [expected]);
                            assert!(
                                project(
                                    &crate::tape_wire::decode::<Request>(&projected.payload)
                                        .unwrap(),
                                    |_| None
                                )
                                .is_err(),
                                "zeroed slots carry no implicit authority"
                            );
                        } else {
                            assert!(projected.is_none(), "numeric root words are not admission");
                        }
                        host::finish(ctx, crate::ordered::Next::Keep).unwrap();
                    }
                    let log = engine::driver(ctx).events.as_ref().unwrap();
                    assert_eq!(
                        log.host_requests
                            .keys()
                            .map(|(_, ordinal)| *ordinal)
                            .collect::<Vec<_>>(),
                        [0, 1, 2, 3]
                    );
                    assert!(log.host_handles.is_empty());
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
        }
    }

    fn identity(word: u64) -> Option<HostAuthority> {
        Some(HostAuthority::Created(HostHandleId {
            operation: crate::ordered::footprint::ObjectId {
                creator: vec![1, 3],
                ordinal: word % 16,
            },
            field: 0,
        }))
    }

    fn command(directory: Option<u64>) -> crate::process::Command {
        crate::process::Command {
            program: "/usr/bin/test".into(),
            args: vec!["λ\0".into()],
            directory,
            env: Some(vec![("NAME".into(), "value".into())]),
            own_group: true,
        }
    }

    #[test]
    fn host_request_projection_covers_nested_command_directories_and_deadline_clocks() {
        let requests = |base: u64| {
            vec![
                Request::ProcessRun {
                    process: base + 1,
                    command: command(Some(base + 2)),
                    stdin: vec![0, 255, 1],
                    max_output: 99,
                    deadline: Some(crate::process::Deadline {
                        clock: base + 3,
                        ticks: 771,
                    }),
                },
                Request::ProcessPipeline {
                    process: base + 1,
                    stages: vec![
                        command(None),
                        command(Some(base + 2)),
                        command(Some(base + 4)),
                    ],
                    stdin: vec![0, 255],
                    max_output: 99,
                    deadline: Some(crate::process::Deadline {
                        clock: base + 3,
                        ticks: 771,
                    }),
                },
                Request::ProcessSpawn {
                    process: base + 1,
                    command: command(Some(base + 2)),
                },
                Request::ProcessSpawnPipeline {
                    process: base + 1,
                    stages: vec![command(Some(base + 2)), command(None)],
                    max_stderr: 99,
                },
                Request::ChildWait {
                    process: base + 1,
                    child: base + 2,
                    deadline: Some(crate::process::Deadline {
                        clock: base + 3,
                        ticks: 771,
                    }),
                },
                Request::GroupWait {
                    process: base + 1,
                    group: base + 2,
                    deadline: Some(crate::process::Deadline {
                        clock: base + 3,
                        ticks: 771,
                    }),
                },
            ]
        };
        for ((first, second), ordinals) in requests(0x1234_0000)
            .into_iter()
            .zip(requests(0x9876_0000))
            .zip([
                vec![1, 2, 3],
                vec![1, 2, 4, 3],
                vec![1, 2],
                vec![1, 2],
                vec![1, 2, 3],
                vec![1, 2, 3],
            ])
        {
            let original = first.clone();
            let a = project(&first, identity).unwrap();
            assert_eq!(a, project(&second, identity).unwrap());
            assert_eq!(
                a.authorities
                    .iter()
                    .map(|authority| match authority {
                        HostAuthority::Created(handle) => handle.operation.ordinal,
                        _ => panic!("created test authority"),
                    })
                    .collect::<Vec<_>>(),
                ordinals
            );
            assert!(project(&first, |_| None).is_err());
            let mut visited = 0;
            assert!(
                project(&first, |word| {
                    visited += 1;
                    (visited != a.authorities.len()).then(|| identity(word).unwrap())
                })
                .is_err()
            );
            assert_eq!(
                visited,
                a.authorities.len(),
                "all nested fields must resolve"
            );
            let mut normalized = crate::tape_wire::decode::<Request>(&a.payload).unwrap();
            assert_eq!(
                project(&normalized, |_| identity(0)).unwrap().payload,
                a.payload
            );
            if let Request::ProcessRun {
                command,
                deadline,
                max_output,
                stdin,
                ..
            } = &mut normalized
            {
                assert_eq!(command.directory, Some(0));
                assert_eq!(deadline.unwrap().clock, 0);
                assert_eq!(deadline.unwrap().ticks, 771);
                assert_eq!(*max_output, 99);
                assert_eq!(stdin, &[0, 255, 1]);
                assert_eq!(command.args, ["λ\0"]);
            }
            assert_eq!(first, original);
        }
    }

    #[test]
    fn host_request_projection_preserves_payloads_while_resolving_all_authority_fields() {
        let requests = |base: u64| {
            vec![
                Request::Print {
                    stdout: base + 1,
                    bytes: vec![0, 255],
                },
                Request::TelemetryEvent {
                    telemetry: base + 1,
                    span: base + 2,
                    payload: vec![0, 255],
                },
                Request::TelemetryBegin {
                    telemetry: base + 1,
                    parent: vec![9; 24],
                    spec: vec![8; 16],
                },
                Request::FileRead {
                    directory: base + 1,
                    file: base + 2,
                    max: u64::MAX,
                },
                Request::FileWrite {
                    directory: base + 1,
                    file: base + 2,
                    bytes: vec![0, 255],
                },
                Request::FileSeek {
                    directory: base + 1,
                    file: base + 2,
                    offset: 771,
                },
                Request::MapView {
                    directory: base + 1,
                    mapped: base + 2,
                },
                Request::WatchNext {
                    directory: base + 1,
                    watcher: base + 2,
                },
                Request::TestCrash {
                    test: base + 1,
                    directory: base + 2,
                    seed: 771,
                },
                Request::TestCheckCase {
                    test: base + 1,
                    condition: true,
                    case: 771,
                },
                Request::NetJoin {
                    net: base + 1,
                    read: base + 2,
                    write: base + 3,
                },
                Request::NetRead {
                    net: base + 1,
                    stream: base + 2,
                    max: u64::MAX,
                },
                Request::Resolve {
                    net: base + 1,
                    host: "example.org".into(),
                    port: 771,
                },
                Request::Close { handle: base + 1 },
                Request::EnvGet {
                    env: base + 1,
                    name: "NAME".into(),
                },
                Request::SleepUntil {
                    clock: base + 1,
                    deadline: 771,
                },
            ]
        };
        for (a, b) in requests(0x1234_0000).iter().zip(requests(0x9876_0000)) {
            let observed = project(a, identity).unwrap();
            assert_eq!(observed, project(&b, identity).unwrap());
            assert!(!observed.authorities.is_empty());
            assert!(project(a, |_| None).is_err());
            let decoded = crate::tape_wire::decode::<Request>(&observed.payload).unwrap();
            assert_eq!(
                project(&decoded, identity).unwrap().payload,
                observed.payload
            );
        }
        let a = Request::Print {
            stdout: 1,
            bytes: vec![0, 255],
        };
        let b = Request::Print {
            stdout: 1,
            bytes: vec![255, 0],
        };
        assert_ne!(
            project(&a, identity).unwrap(),
            project(&b, identity).unwrap()
        );
        let a = Request::FileSeek {
            directory: 1,
            file: 2,
            offset: 771,
        };
        let b = Request::FileSeek {
            directory: 1,
            file: 2,
            offset: 772,
        };
        assert_ne!(
            project(&a, identity).unwrap(),
            project(&b, identity).unwrap()
        );
        for request in [
            Request::RunnerCancel {
                key: [1; 32],
                cause: 3,
            },
            Request::Stopped,
        ] {
            let value = project(&request, |_| panic!("runner payload has no authorities")).unwrap();
            assert!(value.authorities.is_empty());
            assert_eq!(
                crate::tape_wire::decode::<Request>(&value.payload).unwrap(),
                request
            );
        }
        for request in [
            Request::Ready { wait: 1, points: 2 },
            Request::Schedule {
                count: 3,
                choice: 1,
            },
        ] {
            assert!(project(&request, identity).is_err());
        }
    }
}
