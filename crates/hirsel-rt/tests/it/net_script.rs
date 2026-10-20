//! Native scripted peer oracle. Socket vectors are adapted from Rust
//! library/std/src/net/tcp/tests.rs::{timeouts,test_timeout_zero_duration,
//! test_read_with_timeout,shutdown_smoke} (f6c1107f, MIT).
use hirsel_rt::{io::AdapterError, io_net_script::ScriptNet, net::*};
use hirsel_run_config::{NetConnection, NetPeer, NetStep, PeerTls};
fn peer(connections: Vec<NetConnection>) -> NetPeer {
    NetPeer {
        address: "example.com".into(),
        port: 80,
        resolves_to: vec!["127.0.0.1".into(), "::1".into()],
        tls: PeerTls::Plain,
        connections,
    }
}
fn stream(steps: Vec<NetStep>) -> ScriptNet {
    let mut net = ScriptNet::new(vec![peer(vec![NetConnection::Open(steps)])], vec![]).unwrap();
    assert_eq!(
        net.connect(1, "example.com", 80, NetTimeout::Unbounded),
        Ok(Ok(()))
    );
    net
}
#[test]
fn net_script_rust_timeout_vectors_and_unbounded_read_quiescence() {
    let mut net = stream(vec![NetStep::Send(b"hello world".to_vec()), NetStep::Stall]);
    assert_eq!(
        net.set_timeout(1, TimeoutKind::Read, NetTimeout::Nanos(0)),
        Ok(Err(NetError::InvalidInput))
    );
    assert_eq!(
        net.set_timeout(1, TimeoutKind::Write, NetTimeout::Nanos(0)),
        Ok(Err(NetError::InvalidInput))
    );
    assert_eq!(
        net.set_timeout(1, TimeoutKind::Read, NetTimeout::Nanos(15410)),
        Ok(Ok(()))
    );
    assert_eq!(net.read(1, 11), Ok(Ok(b"hello world".to_vec())));
    assert_eq!(net.read(1, 11), Ok(Err(NetError::TimedOut)));
    assert_eq!(net.read(1, 11), Ok(Err(NetError::TimedOut)));
    assert_eq!(
        net.set_timeout(1, TimeoutKind::Read, NetTimeout::Unbounded),
        Ok(Ok(()))
    );
    assert_eq!(net.read(1, 11), Err(AdapterError::Blocked));
}
#[test]
fn net_script_rust_shutdown_smoke_preserves_other_half() {
    let mut net = stream(vec![NetStep::Send(b"reply".to_vec())]);
    assert_eq!(net.shutdown(1, ShutdownHow::Write), Ok(Ok(())));
    assert_eq!(net.write(1, b"request"), Ok(Err(NetError::BrokenPipe)));
    assert_eq!(net.read(1, 10), Ok(Ok(b"reply".to_vec())));
    assert_eq!(net.shutdown(1, ShutdownHow::Read), Ok(Ok(())));
    assert_eq!(net.read(1, 10), Ok(Ok(vec![])));
}
#[test]
fn net_script_chunks_empty_sends_and_implicit_fin() {
    let mut net = stream(vec![
        NetStep::Send(vec![]),
        NetStep::Send(b"abc".to_vec()),
        NetStep::Send(b"de".to_vec()),
    ]);
    assert_eq!(net.read(1, 2), Ok(Ok(b"ab".to_vec())));
    assert_eq!(net.read(1, 10), Ok(Ok(b"c".to_vec())));
    assert_eq!(net.read(1, 10), Ok(Ok(b"de".to_vec())));
    assert_eq!(net.read(1, 10), Ok(Ok(vec![])));
    assert_eq!(net.read(1, 10), Ok(Ok(vec![])));
}
#[test]
fn net_script_buffered_expectations_and_persistent_reset() {
    let mut net = stream(vec![
        NetStep::Expect(b"GET".to_vec()),
        NetStep::Send(b"ok".to_vec()),
        NetStep::Expect(b"end".to_vec()),
        NetStep::Send(b"bye".to_vec()),
    ]);
    assert_eq!(net.write(1, b"GE"), Ok(Ok(2)));
    assert_eq!(net.read(1, 10), Err(AdapterError::Blocked));
    assert_eq!(net.write(1, b"Tend"), Ok(Ok(4)));
    assert_eq!(net.read(1, 10), Ok(Ok(b"ok".to_vec())));
    assert_eq!(net.read(1, 10), Ok(Ok(b"bye".to_vec())));
    let mut net = stream(vec![
        NetStep::Expect(b"yes".to_vec()),
        NetStep::Send(b"ok".to_vec()),
    ]);
    assert_eq!(net.write(1, b"no!"), Ok(Ok(3)));
    assert_eq!(net.read(1, 10), Ok(Err(NetError::ConnectionReset)));
    assert_eq!(net.write(1, b"yes"), Ok(Err(NetError::ConnectionReset)));
    assert_eq!(net.peer_addr(1), Ok(Err(NetError::ConnectionReset)));
}
#[test]
fn net_script_connection_queue_timeout_refusal_and_exhaustion() {
    let mut net = ScriptNet::new(
        vec![peer(vec![
            NetConnection::Unanswered,
            NetConnection::Refused,
            NetConnection::Unanswered,
            NetConnection::Open(vec![]),
        ])],
        vec![],
    )
    .unwrap();
    assert_eq!(
        net.connect(1, "example.com", 80, NetTimeout::Nanos(0)),
        Ok(Err(NetError::InvalidInput))
    );
    assert_eq!(
        net.connect(1, "example.com", 80, NetTimeout::Nanos(1)),
        Ok(Err(NetError::TimedOut))
    );
    assert_eq!(
        net.connect(1, "example.com", 80, NetTimeout::Unbounded),
        Ok(Err(NetError::ConnectionRefused))
    );
    assert_eq!(
        net.connect(1, "example.com", 80, NetTimeout::Unbounded),
        Err(AdapterError::Blocked)
    );
    assert_eq!(
        net.connect(1, "example.com", 80, NetTimeout::Unbounded),
        Ok(Ok(()))
    );
    assert_eq!(
        net.connect(2, "example.com", 80, NetTimeout::Unbounded),
        Ok(Err(NetError::ConnectionRefused))
    );
    net.close(1);
    assert_eq!(net.read(1, 10), Err(AdapterError::InvalidHandle(1)));
}
#[test]
fn net_script_resolver_order_peer_address_and_duplicate_rejection() {
    let p = peer(vec![]);
    assert!(
        matches!(ScriptNet::new(vec![p.clone(),p.clone()], vec![]),Err(AdapterError::Unsupported(s)) if s.contains("entries 0 and 1"))
    );
    let mut net = ScriptNet::new(vec![p], vec![]).unwrap();
    let addresses = net.resolve("example.com", 80).unwrap().unwrap();
    assert_eq!(
        addresses
            .iter()
            .map(|a| a.ip.to_string())
            .collect::<Vec<_>>(),
        vec!["127.0.0.1", "::1"]
    );
    assert_eq!(net.resolve("other.com", 80), Ok(Err(NetError::UnknownHost)));
    let mut p = peer(vec![NetConnection::Open(vec![])]);
    p.resolves_to.clear();
    let mut net = ScriptNet::new(vec![p], vec![]).unwrap();
    net.connect(1, "example.com", 80, NetTimeout::Unbounded)
        .unwrap()
        .unwrap();
    assert_eq!(net.peer_addr(1), Ok(Err(NetError::UnknownHost)));
}
#[test]
fn net_script_tls_checks_names_and_ignores_roots() {
    let roots = RootsDigest::Pem([0; 32]);
    let mut p = peer(vec![NetConnection::Open(vec![])]);
    p.tls = PeerTls::Certificate(vec!["example.com".into()]);
    let mut net = ScriptNet::new(vec![p], vec![]).unwrap();
    net.connect(1, "example.com", 80, NetTimeout::Unbounded)
        .unwrap()
        .unwrap();
    assert_eq!(
        net.tls_connect(hirsel_rt::net_tls::engine(), 1, "other.com", &roots, None),
        Ok(Err(NetError::Tls(TlsFailure::NameMismatch)))
    );
    assert_eq!(
        net.tls_connect(hirsel_rt::net_tls::engine(), 1, "example.com", &roots, None),
        Ok(Ok(()))
    );
    let mut net = stream(vec![]);
    assert_eq!(
        net.tls_connect(hirsel_rt::net_tls::engine(), 1, "example.com", &roots, None),
        Ok(Err(NetError::Tls(TlsFailure::HandshakeFailure)))
    );
}

#[test]
fn net_script_literal_addresses_write_all_close_and_an_unscripted_listener() {
    let mut p = peer(vec![NetConnection::Open(vec![])]);
    p.address = "127.0.0.1".into();
    p.resolves_to.clear();
    let mut net = ScriptNet::new(vec![p], vec![]).unwrap();
    let address = SocketAddr {
        ip: "127.0.0.1".parse().unwrap(),
        port: 80,
    };
    assert_eq!(
        net.connect_addr(1, address, NetTimeout::Unbounded),
        Ok(Ok(()))
    );
    assert_eq!(net.peer_addr(1), Ok(Ok(address)));
    assert_eq!(
        net.set_timeout(1, TimeoutKind::Write, NetTimeout::Nanos(1)),
        Ok(Ok(()))
    );
    assert_eq!(net.write_all(1, b"bytes"), Ok(Ok(())));
    assert_eq!(net.tls_close(1), Ok(Ok(())));
    assert_eq!(net.shutdown(1, ShutdownHow::Both), Ok(Ok(())));
    // A listen address with no scripted listener has no clients.
    assert_eq!(net.listen(2, address), Ok(Ok(())));
    assert_eq!(net.listen(4, address), Ok(Err(NetError::AddrInUse)));
    assert_eq!(net.local_addr(2), Ok(Ok(address)));
    assert_eq!(net.accept(2, 3), Ok(Err(NetError::ConnectionAborted)));
}

fn script_adapter() -> hirsel_rt::io_real::RealIo {
    let mut io = hirsel_rt::io_real::RealIo::from_pinned_mounts(vec![], std::io::sink()).unwrap();
    crate::support::host(&mut io);
    io
}

#[test]
fn repeated_tls_upgrade_is_invalid_and_consumes_the_script_stream() {
    use hirsel_rt::io::{Adapter, AdapterError, Completion as C, Request as R};
    let mut io = script_adapter();
    io.test_context(7);
    let mut p = peer(vec![NetConnection::Open(vec![])]);
    p.tls = PeerTls::Certificate(vec!["example.com".into()]);
    assert_eq!(
        io.call(R::TestNet {
            test: 1,
            peers: vec![p],
            listeners: vec![]
        }),
        Ok(C::Handle(Ok(2)))
    );
    assert_eq!(
        io.call(R::Connect {
            net: 2,
            host: "example.com".into(),
            port: 80,
            timeout: NetTimeout::Unbounded
        }),
        Ok(C::NetHandle(Ok(3)))
    );
    let upgrade = R::TlsConnect {
        net: 2,
        stream: 3,
        server_name: "example.com".into(),
        roots: RootsDigest::WebPki("webpki-roots@1.0.9".into()),
        identity: None,
    };
    assert_eq!(io.call(upgrade.clone()), Ok(C::NetUnit(Ok(()))));
    let mut repeated = upgrade;
    if let R::TlsConnect { server_name, .. } = &mut repeated {
        *server_name = "other.com".into();
    }
    assert_eq!(
        io.call(repeated),
        Ok(C::NetUnit(Err(NetError::InvalidInput)))
    );
    assert_eq!(
        io.call(R::NetRead {
            net: 2,
            stream: 3,
            max: 1
        }),
        Err(AdapterError::InvalidHandle(3))
    );
}

#[test]
fn tls_only_script_can_set_timeouts_before_a_stalled_handshake() {
    use hirsel_rt::io::{Adapter, Completion as C, Request as R};
    let mut io = script_adapter();
    io.test_context(7);
    let mut p = peer(vec![NetConnection::Open(vec![NetStep::Stall])]);
    p.tls = PeerTls::Certificate(vec!["example.com".into()]);
    assert_eq!(
        io.call(R::TestNet {
            test: 1,
            peers: vec![p],
            listeners: vec![]
        }),
        Ok(C::Handle(Ok(2)))
    );
    assert_eq!(
        io.call(R::Connect {
            net: 2,
            host: "example.com".into(),
            port: 80,
            timeout: NetTimeout::Unbounded
        }),
        Ok(C::NetHandle(Ok(3)))
    );
    for which in [TimeoutKind::Read, TimeoutKind::Write] {
        assert_eq!(
            io.call(R::SetTimeout {
                net: 2,
                stream: 3,
                which,
                timeout: NetTimeout::Nanos(1)
            }),
            Ok(C::NetUnit(Ok(())))
        );
    }
    assert!(matches!(
        io.call(R::PeerAddr { net: 2, stream: 3 }),
        Ok(C::Address(Ok(_)))
    ));
    assert_eq!(
        io.call(R::TlsConnect {
            net: 2,
            stream: 3,
            server_name: "example.com".into(),
            roots: RootsDigest::WebPki("webpki-roots@1.0.9".into()),
            identity: None,
        }),
        Ok(C::NetUnit(Err(NetError::TimedOut)))
    );
    assert!(!io.handles().observed().contains_key(&3));
}
#[test]
fn net_script_test_worlds_are_isolated_and_recordings_rederive_script_outcomes() {
    use hirsel_rt::io::{Adapter, Completion as C, Request as R, Tree};
    use hirsel_rt::{
        io_sim::SimIo,
        launch::{parse_tape, render_tape},
    };
    let mut real = script_adapter();
    real.test_context(7);
    let peers = vec![peer(vec![NetConnection::Open(vec![
        NetStep::Expect(b"ping".to_vec()),
        NetStep::Send(b"reply".to_vec()),
    ])])];
    let make = R::TestNet {
        test: 1,
        peers,
        listeners: vec![],
    };
    assert_eq!(real.call(make.clone()), Ok(C::Handle(Ok(2))));
    assert_eq!(real.call(make.clone()), Ok(C::Handle(Ok(3))));
    assert_eq!(
        real.call(R::NetNarrow {
            net: 2,
            rules: vec![]
        }),
        Ok(C::NetHandle(Ok(4)))
    );
    let connect = |net| R::Connect {
        net,
        host: "example.com".into(),
        port: 80,
        timeout: NetTimeout::Unbounded,
    };
    assert_eq!(
        real.call(connect(4)),
        Ok(C::NetHandle(Err(NetError::NotGranted)))
    );
    for (net, stream) in [(2, 5), (3, 6)] {
        assert_eq!(real.call(connect(net)), Ok(C::NetHandle(Ok(stream))));
        assert_eq!(
            real.call(R::NetWriteAll {
                net,
                stream,
                bytes: b"ping".to_vec()
            }),
            Ok(C::NetUnit(Ok(())))
        );
        assert_eq!(
            real.call(R::NetRead {
                net,
                stream,
                max: 10
            }),
            Ok(C::NetBytes(Ok(b"reply".to_vec())))
        );
        assert_eq!(real.call(R::Close { handle: stream }), Ok(C::Unit(Ok(()))));
    }
    let tape = real.tape().to_vec();
    assert_eq!(parse_tape(&render_tape(&tape).unwrap()).unwrap(), tape);
    let mut sim = SimIo::new(Tree::default(), tape.clone());
    crate::support::host(&mut sim);
    sim.test_context(7);
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.handles(), real.handles());
    let read_at = tape
        .iter()
        .position(|e| matches!(e.request, R::NetRead { .. }))
        .unwrap();
    let mut forged = tape.clone();
    forged[read_at].completion = C::NetBytes(Ok(b"forged".to_vec()));
    let mut sim = SimIo::new(Tree::default(), forged);
    crate::support::host(&mut sim);
    sim.test_context(7);
    for event in &tape[..read_at] {
        sim.call(event.request.clone()).unwrap();
    }
    let handles = sim.handles().clone();
    assert_eq!(
        sim.call(tape[read_at].request.clone()),
        Err(AdapterError::ReplayMismatch(read_at))
    );
    assert_eq!(sim.position(), read_at);
    assert_eq!(sim.handles(), &handles);
}
#[test]
fn net_script_quiescence_retains_the_native_prefix_and_live_stream_without_cleanup() {
    use hirsel_rt::io::{Adapter, Completion as C, Request as R};
    let mut real = script_adapter();
    real.test_context(7);
    assert_eq!(
        real.call(R::TestNet {
            test: 1,
            peers: vec![peer(vec![NetConnection::Open(vec![
                NetStep::Send(b"prefix".to_vec()),
                NetStep::Stall,
            ])])],
            listeners: vec![]
        }),
        Ok(C::Handle(Ok(2)))
    );
    assert_eq!(
        real.call(R::Connect {
            net: 2,
            host: "example.com".into(),
            port: 80,
            timeout: NetTimeout::Unbounded
        }),
        Ok(C::NetHandle(Ok(3)))
    );
    assert_eq!(
        real.call(R::NetRead {
            net: 2,
            stream: 3,
            max: 20
        }),
        Ok(C::NetBytes(Ok(b"prefix".to_vec())))
    );
    let handles = real.handles().clone();
    let tape = real.tape().to_vec();
    assert_eq!(
        real.call(R::NetRead {
            net: 2,
            stream: 3,
            max: 20
        }),
        Err(AdapterError::Blocked)
    );
    assert_eq!(real.handles(), &handles);
    assert_eq!(real.tape(), &tape);
}
#[test]
fn net_script_configured_peers_do_not_widen_explicit_root_rules() {
    use hirsel_rt::io::{Adapter, Completion as C, Request as R};
    let mut real = script_adapter();
    real.set_net_script(vec![peer(vec![NetConnection::Open(vec![])])], vec![])
        .unwrap();
    assert_eq!(real.call(R::Net { io: 1 }), Ok(C::Handle(Ok(2))));
    assert_eq!(
        real.call(R::Connect {
            net: 2,
            host: "example.com".into(),
            port: 80,
            timeout: NetTimeout::Unbounded
        }),
        Ok(C::NetHandle(Err(NetError::NotGranted)))
    );
}

#[test]
fn net_script_configured_root_executes_and_replays_with_explicit_policy() {
    use hirsel_rt::io::{Adapter, Completion as C, Request as R, Tree};
    use hirsel_rt::io_sim::SimIo;
    let peers = vec![peer(vec![NetConnection::Open(vec![NetStep::Send(
        b"root".to_vec(),
    )])])];
    let rules = [NetRule {
        address: "example.com".into(),
        port: 80,
        operation: NetOperation::Connect,
    }]
    .into();
    let mut real = script_adapter();
    real.set_net_script(peers.clone(), vec![]).unwrap();
    real.set_net_rules(rules);
    assert_eq!(real.call(R::Net { io: 1 }), Ok(C::Handle(Ok(2))));
    assert_eq!(
        real.call(R::Connect {
            net: 2,
            host: "example.com".into(),
            port: 80,
            timeout: NetTimeout::Unbounded
        }),
        Ok(C::NetHandle(Ok(3)))
    );
    assert_eq!(
        real.call(R::NetRead {
            net: 2,
            stream: 3,
            max: 9
        }),
        Ok(C::NetBytes(Ok(b"root".to_vec())))
    );
    let tape = real.tape().to_vec();
    let mut sim = SimIo::new(Tree::default(), tape.clone());
    crate::support::host(&mut sim);
    sim.set_net_script(peers, vec![]).unwrap();
    sim.set_net_rules(
        [NetRule {
            address: "example.com".into(),
            port: 80,
            operation: NetOperation::Connect,
        }]
        .into(),
    );
    for event in tape {
        assert_eq!(sim.call(event.request), Ok(event.completion));
    }
    sim.finish().unwrap();
}

#[test]
fn ephemeral_listen_completions_record_assigned_ports_and_refuse_altered_replay() {
    use hirsel_rt::io::{Adapter, Completion as C, Request as R, Tree};
    use hirsel_rt::io_sim::SimIo;
    use hirsel_run_config::{NetClient, NetListener};
    let address = |port| SocketAddr {
        ip: std::net::Ipv4Addr::LOCALHOST.into(),
        port,
    };
    let listeners = vec![
        NetListener {
            address: "127.0.0.1".into(),
            port: 49152,
            clients: vec![],
        },
        NetListener {
            address: "127.0.0.1".into(),
            port: 0,
            clients: vec![NetClient {
                address: "192.0.2.1".into(),
                port: 40001,
                tls: hirsel_run_config::PeerTls::Plain,
                steps: vec![NetStep::Send(b"hi".to_vec())],
            }],
        },
    ];
    let rules = [NetRule {
        address: "127.0.0.1".into(),
        port: 0,
        operation: NetOperation::Listen,
    }]
    .into();
    let mut io = script_adapter();
    io.set_net_rules(rules);
    io.set_net_script(vec![], listeners.clone()).unwrap();
    assert_eq!(io.call(R::Net { io: 1 }), Ok(C::Handle(Ok(2))));
    assert_eq!(
        io.call(R::Listen {
            net: 2,
            addr: address(0)
        }),
        Ok(C::Listened(Ok((3, address(49153)))))
    );
    assert_eq!(
        io.call(R::LocalAddr {
            net: 2,
            listener: 3
        }),
        Ok(C::Address(Ok(address(49153))))
    );
    let from = SocketAddr {
        ip: "192.0.2.1".parse().unwrap(),
        port: 40001,
    };
    assert_eq!(
        io.call(R::Accept {
            net: 2,
            listener: 3
        }),
        Ok(C::Accepted(Ok((4, from))))
    );
    assert_eq!(
        io.call(R::NetRead {
            net: 2,
            stream: 4,
            max: 2
        }),
        Ok(C::NetBytes(Ok(b"hi".to_vec())))
    );
    assert_eq!(io.call(R::Close { handle: 4 }), Ok(C::Unit(Ok(()))));
    assert_eq!(io.call(R::Close { handle: 3 }), Ok(C::Unit(Ok(()))));
    assert_eq!(
        io.call(R::Listen {
            net: 2,
            addr: address(0)
        }),
        Ok(C::Listened(Ok((5, address(49154)))))
    );
    assert_eq!(
        io.call(R::Accept {
            net: 2,
            listener: 5
        }),
        Ok(C::Accepted(Err(NetError::ConnectionAborted)))
    );
    assert_eq!(io.call(R::Close { handle: 5 }), Ok(C::Unit(Ok(()))));
    let text = hirsel_rt::launch::render_tape(io.tape()).unwrap();
    let tape = hirsel_rt::launch::parse_tape(&text).unwrap();
    assert_eq!(tape, io.tape());
    for altered in [false, true] {
        let mut input = tape.clone();
        if altered {
            input[1].completion = C::Listened(Ok((3, address(49155))));
        }
        let mut sim = SimIo::new(Tree::default(), input);
        crate::support::host(&mut sim);
        sim.set_net_rules(
            [NetRule {
                address: "127.0.0.1".into(),
                port: 0,
                operation: NetOperation::Listen,
            }]
            .into(),
        );
        sim.set_net_script(vec![], listeners.clone()).unwrap();
        for (index, event) in tape.iter().enumerate() {
            let result = sim.call(event.request.clone());
            if altered && index == 1 {
                assert!(result.is_err());
                break;
            }
            assert_eq!(result, Ok(event.completion.clone()));
        }
        if !altered {
            sim.finish().unwrap();
        }
    }
}
