use super::*;
use crate::io_keys::{Key, Kind, NAMESPACE};

fn net(ordinal: u64, resource: u32, ports: Option<u32>) -> Envelope {
    Envelope {
        result: Ok(Completion::NetHandle(Ok(0))),
        output_count: None,
        created: vec![Created {
            path: vec![1],
            ordinal,
            resource,
            kind: Kind::Other,
            process_keys: None,
            net_ports: ports,
        }],
    }
}
#[test]
fn net_creation_requires_port_metadata_before_any_handle_is_installed() {
    for request in [
        Request::Net { io: 1 },
        Request::NetNarrow {
            net: 2,
            rules: vec![],
        },
    ] {
        let mut handles = handles::Handles::default();
        assert!(handles.substitute_for(&request, net(1, 17, None)).is_err());
        assert!(handles.identity(2).is_err());
        let ordinal = if matches!(request, Request::NetNarrow { .. }) {
            assert!(
                handles
                    .substitute_for(&request, net(1, 17, Some(18)))
                    .is_err()
            );
            handles
                .substitute_for(&Request::Net { io: 1 }, net(1, 17, Some(18)))
                .unwrap();
            2
        } else {
            1
        };
        assert!(
            handles
                .substitute_for(&request, net(ordinal, 17, Some(18)))
                .is_ok()
        );
        assert_eq!(handles.net_ports(2).unwrap(), Key(18));
    }
    let mut handles = handles::Handles::default();
    let request = Request::EnvNarrow {
        env: crate::io::ENV_HANDLE,
        names: vec![],
    };
    assert!(
        handles
            .substitute_for(&request, net(1, 17, Some(18)))
            .is_err()
    );
    assert!(handles.identity(2).is_err());
    let mut failed = net(1, 17, None);
    failed.created.clear();
    failed.result = Ok(Completion::Handle(Err(crate::io::IoError::NotGranted)));
    assert!(
        handles
            .substitute_for(&Request::Net { io: 1 }, failed)
            .is_ok()
    );
}

#[test]
fn net_grant_substitution_rejects_changed_world_before_mapping() {
    for request in [
        Request::Net { io: 1 },
        Request::NetNarrow {
            net: 2,
            rules: vec![],
        },
    ] {
        let mut handles = handles::Handles::default();
        handles
            .substitute_for(&Request::Net { io: 1 }, net(1, 17, Some(18)))
            .unwrap();
        assert!(
            handles
                .substitute_for(&request, net(2, 19, Some(20)))
                .is_err()
        );
        assert!(
            handles.identity(3).is_err(),
            "a refused domain must not install a handle"
        );
        assert!(
            handles
                .substitute_for(&request, net(2, 17, Some(18)))
                .is_ok()
        );
        assert_eq!(handles.net_ports(3).unwrap(), Key(18));
    }
}

#[test]
fn real_repeated_and_narrowed_net_grants_share_the_registered_port_domain() {
    use crate::io_concurrent::ConcurrentAdapter;
    let temp = tempfile::tempdir().unwrap();
    let real = crate::io_real::RealIo::grant(temp.path()).unwrap();
    let io = real
        .into_concurrent()
        .unwrap_or_else(|_| panic!("Send streams"));
    let mut grants = Vec::new();
    for request in [Request::Net { io: 1 }, Request::Net { io: 1 }] {
        let Ok(Completion::Handle(Ok(net))) = io.call(request) else {
            panic!("Net")
        };
        grants.push(net);
    }
    let Ok(Completion::NetHandle(Ok(narrowed))) = io.call(Request::NetNarrow {
        net: grants[0],
        rules: vec![],
    }) else {
        panic!("Net narrow")
    };
    grants.push(narrowed);
    let resource = io.identity(grants[0]).unwrap();
    let ports = io.net_ports(grants[0]).unwrap();
    assert_ne!(resource.0, ports);
    for net in grants {
        assert_eq!(io.identity(net), Some(resource));
        assert_eq!(io.net_ports(net), Some(ports));
        let request = Request::Listen {
            net,
            addr: crate::net::SocketAddr::of(([127, 0, 0, 1], 0).into()),
        };
        assert_eq!(
            io.keys(&request),
            KeySet::new(vec![(ports, Mode::Exclusive)])
        );
    }
    assert!(io.net_ports(1).is_none());
}
#[test]
fn net_domains_validate_before_mapping_and_aliases_preserve_ports() {
    let mut handles = handles::Handles::default();
    for ports in [0, 1, 2, 3, 17] {
        assert!(handles.substitute(net(1, 17, Some(ports))).is_err());
        assert!(handles.net_ports(2).is_err());
    }
    let Ok(Completion::NetHandle(Ok(first))) = handles.substitute(net(1, 17, Some(18))).unwrap()
    else {
        panic!("Net")
    };
    for (resource, ports) in [(17, None), (17, Some(19)), (19, Some(18))] {
        assert!(handles.substitute(net(2, resource, ports)).is_err());
    }
    let Ok(Completion::NetHandle(Ok(alias))) = handles.substitute(net(2, 17, Some(18))).unwrap()
    else {
        panic!("Net alias")
    };
    assert_ne!(first, alias);
    assert_eq!(handles.net_ports(first).unwrap(), Key(18));
    assert_eq!(handles.net_ports(first), handles.net_ports(alias));
    let request = Request::Listen {
        net: alias,
        addr: crate::net::SocketAddr::of(([127, 0, 0, 1], 0).into()),
    };
    assert_eq!(
        native::replay_keys(&handles, &request).unwrap(),
        KeySet::new(vec![(Key(18), Mode::Exclusive)])
    );
    let request = Request::UnixListen {
        net: alias,
        path: "/tmp/socket".into(),
    };
    assert_eq!(
        native::replay_keys(&handles, &request).unwrap(),
        KeySet::new(vec![
            (NAMESPACE, Mode::Exclusive),
            (Key(18), Mode::Exclusive)
        ])
    );
    let request = Request::NetRead {
        net: 999,
        stream: first,
        max: 1,
    };
    assert!(native::replay_keys(&handles, &request).is_err());
    // An object with the same scalar shape is never a Net without its domain.
    let Ok(Completion::NetHandle(Ok(other))) = handles.substitute(net(3, 20, None)).unwrap() else {
        panic!("other")
    };
    assert!(
        native::replay_keys(
            &handles,
            &Request::Resolve {
                net: other,
                host: "localhost".into(),
                port: 80
            }
        )
        .is_err()
    );
}
#[test]
fn initial_env_requires_admission_and_split_network_positions_preserve_parent() {
    let env = Request::EnvGet {
        env: crate::io::ENV_HANDLE,
        name: "MODE".into(),
    };
    for admitted in [false, true] {
        let keys = native::request_keys_with_domains(
            &env,
            |_| Err(Stop::Diverged("no handle".into())),
            |_| Err(Stop::Diverged("no process".into())),
            |_| Err(Stop::Diverged("no net".into())),
            admitted,
        );
        if admitted {
            assert_eq!(keys.unwrap(), KeySet::default());
        } else {
            assert!(keys.is_err());
        }
    }
    let request = Request::NetRead {
        net: 4,
        stream: 5,
        max: 8,
    };
    let keys = native::request_keys_with_domains(
        &request,
        |word| match word {
            5 => Ok((Key(20), Kind::Half { stream: Key(19) })),
            _ => Err(Stop::Diverged("unknown".into())),
        },
        |_| Err(Stop::Diverged("no process".into())),
        |word| {
            if word == 4 {
                Ok(Key(18))
            } else {
                Err(Stop::Diverged("no Net".into()))
            }
        },
        false,
    )
    .unwrap();
    assert_eq!(
        keys,
        KeySet::new(vec![(Key(19), Mode::Shared), (Key(20), Mode::Exclusive)])
    );
    let authorities = super::requests::Authorities::default();
    authorities.root(false);
    assert!(authorities.project(&env).is_err());
    let authorities = super::requests::Authorities::default();
    authorities.root(true);
    let bytes = authorities.project(&env).unwrap();
    let (_, ids): (Vec<u8>, Vec<super::requests::Authority>) =
        crate::tape_wire::decode(&bytes).unwrap();
    assert_eq!(ids, [super::requests::Authority::RootEnv]);
}

#[test]
fn restored_stream_words_get_the_join_operations_result_identity() {
    let authorities = super::requests::Authorities::default();
    authorities.root(false);
    for op in [1, 3] {
        // A successful NetJoin may restore its original stream's physical word.
        authorities
            .observe(&[1], op, &Ok(Completion::NetHandle(Ok(7))))
            .unwrap();
    }
    let bytes = authorities.project(&Request::Close { handle: 7 }).unwrap();
    let (_, ids): (Vec<u8>, Vec<super::requests::Authority>) =
        crate::tape_wire::decode(&bytes).unwrap();
    assert_eq!(
        ids,
        [super::requests::Authority::Created {
            path: vec![1],
            op: 3,
            field: 0
        }]
    );
}
