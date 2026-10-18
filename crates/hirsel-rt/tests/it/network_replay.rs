//! Independent interpreter replay of native typed network recordings.
use crate::support::io::replay_interpreter;
use hirsel_rt::{io::*, io_real::RealIo};
use hirsel_run_config::{NetConnection, NetPeer, NetStep, PeerTls};
use std::collections::BTreeMap;

fn tls_record(peer_tls: PeerTls) -> RealIo {
    tls_record_with_roots(
        peer_tls,
        RootsDigest::WebPki("webpki-roots@1.0.9".into()),
        None,
    )
}
fn tls_record_with_roots(peer_tls: PeerTls, roots: RootsDigest, pem: Option<&[u8]>) -> RealIo {
    let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    io.set_net_rules(
        [NetRule {
            address: "127.0.0.1".into(),
            port: 443,
            operation: NetOperation::Connect,
        }]
        .into(),
    );
    io.set_net_script(
        vec![NetPeer {
            address: "127.0.0.1".into(),
            port: 443,
            resolves_to: vec![],
            tls: peer_tls,
            connections: vec![NetConnection::Open(vec![
                NetStep::Send(b"tls".to_vec()),
                NetStep::Close,
            ])],
        }],
        vec![],
    )
    .unwrap();
    let Completion::Handle(Ok(net)) = io.call(Request::Net { io: 1 }).unwrap() else {
        panic!()
    };
    let Completion::NetHandle(Ok(stream)) = io
        .call(Request::Connect {
            net,
            host: "127.0.0.1".into(),
            port: 443,
            timeout: NetTimeout::Unbounded,
        })
        .unwrap()
    else {
        panic!()
    };
    if let Some(pem) = pem {
        io.tls_roots(stream, pem);
    }
    let completion = io
        .call(Request::TlsConnect {
            net,
            stream,
            server_name: "localhost".into(),
            roots,
        })
        .unwrap();
    if completion == Completion::NetUnit(Ok(())) {
        for expected in [b"tl".as_slice(), b"s", b""] {
            assert_eq!(
                io.call(Request::NetRead {
                    net,
                    stream,
                    max: 2
                })
                .unwrap(),
                Completion::NetBytes(Ok(expected.to_vec()))
            );
        }
        assert_eq!(
            io.call(Request::TlsClose { net, stream }).unwrap(),
            Completion::NetUnit(Ok(()))
        );
    }
    io
}
#[test]
fn scripted_tls_success_close_name_mismatch_and_handshake_failure_replay_independently() {
    let tree = Tree::new(BTreeMap::new()).unwrap();
    for tls in [
        PeerTls::Plain,
        PeerTls::Certificate(vec!["localhost".into()]),
        PeerTls::Certificate(vec!["other.example".into()]),
    ] {
        let io = tls_record(tls);
        replay_interpreter(io.tape(), &tree, &tree, io.handles().observed(), b"");
    }
}
pub const FAILURES: [TlsFailure; 10] = [
    TlsFailure::UnknownIssuer,
    TlsFailure::Expired,
    TlsFailure::NotYetValid,
    TlsFailure::NameMismatch,
    TlsFailure::Revoked,
    TlsFailure::BadCertificate,
    TlsFailure::AlertReceived(40),
    TlsFailure::HandshakeFailure,
    TlsFailure::PeerMisbehaved,
    TlsFailure::Truncated,
];
fn handshake(io: &RealIo) -> Completion {
    io.tape()
        .iter()
        .find(|event| matches!(event.request, Request::TlsConnect { .. }))
        .unwrap()
        .completion
        .clone()
}
#[test]
fn every_failing_peer_handshake_records_its_failure_and_replays_independently() {
    use crate::support::io::{ReplayFixture, replay_interpreter_with_fixture};
    let tree = Tree::new(BTreeMap::new()).unwrap();
    for failure in FAILURES {
        let io = tls_record(PeerTls::Fails(failure.into()));
        assert_eq!(
            handshake(&io),
            Completion::NetUnit(Err(NetError::Tls(failure)))
        );
        replay_interpreter(io.tape(), &tree, &tree, io.handles().observed(), b"");
        // The interpreter reproduces the fixture's failure, not the comparison tape's.
        let mut altered = io.tape().to_vec();
        altered
            .iter_mut()
            .find(|event| matches!(event.request, Request::TlsConnect { .. }))
            .unwrap()
            .completion = Completion::NetUnit(Err(NetError::Tls(TlsFailure::AlertReceived(41))));
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                replay_interpreter_with_fixture(
                    &altered,
                    ReplayFixture {
                        events: io.tape(),
                        pem_roots: &[],
                    },
                    &tree,
                    &tree,
                    io.handles().observed(),
                    b"",
                    &BTreeMap::new(),
                )
            }))
            .is_err(),
            "{failure:?}"
        );
    }
}
#[test]
fn a_truncate_step_fails_secured_reads_and_ends_plaintext_reads() {
    let tree = Tree::new(BTreeMap::new()).unwrap();
    for tls in [true, false] {
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        io.set_net_rules(
            [NetRule {
                address: "127.0.0.1".into(),
                port: 443,
                operation: NetOperation::Connect,
            }]
            .into(),
        );
        io.set_net_script(
            vec![NetPeer {
                address: "127.0.0.1".into(),
                port: 443,
                resolves_to: vec![],
                tls: PeerTls::Certificate(vec!["localhost".into()]),
                connections: vec![NetConnection::Open(vec![
                    NetStep::Send(b"tls".to_vec()),
                    NetStep::Truncate,
                    NetStep::Send(b"unreached".to_vec()),
                ])],
            }],
            vec![],
        )
        .unwrap();
        let Completion::Handle(Ok(net)) = io.call(Request::Net { io: 1 }).unwrap() else {
            panic!()
        };
        let Completion::NetHandle(Ok(stream)) = io
            .call(Request::Connect {
                net,
                host: "127.0.0.1".into(),
                port: 443,
                timeout: NetTimeout::Unbounded,
            })
            .unwrap()
        else {
            panic!()
        };
        if tls {
            assert_eq!(
                io.call(Request::TlsConnect {
                    net,
                    stream,
                    server_name: "localhost".into(),
                    roots: RootsDigest::WebPki("webpki-roots@1.0.9".into()),
                })
                .unwrap(),
                Completion::NetUnit(Ok(()))
            );
        }
        let end = if tls {
            Err(NetError::Tls(TlsFailure::Truncated))
        } else {
            Ok(vec![])
        };
        for expected in [Ok(b"tls".to_vec()), end.clone(), end] {
            assert_eq!(
                io.call(Request::NetRead {
                    net,
                    stream,
                    max: 8
                })
                .unwrap(),
                Completion::NetBytes(expected)
            );
        }
        assert_eq!(
            io.call(Request::NetWrite {
                net,
                stream,
                bytes: b"after".to_vec()
            })
            .unwrap(),
            Completion::Count(Ok(5))
        );
        replay_interpreter(io.tape(), &tree, &tree, io.handles().observed(), b"");
    }
}

#[test]
fn pem_roots_require_the_original_bundle_matching_the_recorded_digest() {
    use crate::support::io::{ReplayFixture, replay_interpreter_with_fixture};
    use sha2::{Digest, Sha256};
    // Both engines parse the original trust bundle before the handshake. The tape
    // records its digest; the same raw bundle must be supplied on both executions.
    let pem = include_bytes!("tls-data/ca.cert");
    let io = tls_record_with_roots(
        PeerTls::Certificate(vec!["localhost".into()]),
        RootsDigest::Pem(Sha256::digest(pem).into()),
        Some(pem),
    );
    let tree = Tree::new(BTreeMap::new()).unwrap();
    let replay = |pem: &[u8]| {
        replay_interpreter_with_fixture(
            io.tape(),
            ReplayFixture {
                events: io.tape(),
                pem_roots: &[pem],
            },
            &tree,
            &tree,
            io.handles().observed(),
            b"",
            &BTreeMap::new(),
        )
    };
    replay(pem);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| replay(b"wrong bundle"))).is_err()
    );
}
#[test]
fn certificate_names_are_merged_independently_of_tls_event_order() {
    let tree = Tree::new(BTreeMap::new()).unwrap();
    for names in [
        ["localhost", "other.example"],
        ["other.example", "localhost"],
    ] {
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        io.set_net_rules(
            [NetRule {
                address: "127.0.0.1".into(),
                port: 443,
                operation: NetOperation::Connect,
            }]
            .into(),
        );
        io.set_net_script(
            vec![NetPeer {
                address: "127.0.0.1".into(),
                port: 443,
                resolves_to: vec![],
                tls: PeerTls::Certificate(vec!["localhost".into()]),
                connections: vec![NetConnection::Open(vec![]); 2],
            }],
            vec![],
        )
        .unwrap();
        let Completion::Handle(Ok(net)) = io.call(Request::Net { io: 1 }).unwrap() else {
            panic!()
        };
        for name in names {
            let Completion::NetHandle(Ok(stream)) = io
                .call(Request::Connect {
                    net,
                    host: "127.0.0.1".into(),
                    port: 443,
                    timeout: NetTimeout::Unbounded,
                })
                .unwrap()
            else {
                panic!()
            };
            let result = io
                .call(Request::TlsConnect {
                    net,
                    stream,
                    server_name: name.into(),
                    roots: RootsDigest::WebPki("webpki-roots@1.0.9".into()),
                })
                .unwrap();
            let expected = if name == "localhost" {
                Ok(())
            } else {
                Err(NetError::Tls(TlsFailure::NameMismatch))
            };
            assert_eq!(result, Completion::NetUnit(expected.clone()));
            if expected.is_ok() {
                assert_eq!(
                    io.call(Request::TlsClose { net, stream }).unwrap(),
                    Completion::NetUnit(Ok(()))
                );
            }
        }
        replay_interpreter(io.tape(), &tree, &tree, io.handles().observed(), b"");
        // A fixed peer certificate cannot both accept and reject one name.
        let mut conflicting = io.tape().to_vec();
        for event in &mut conflicting {
            if event.completion == Completion::NetUnit(Err(NetError::Tls(TlsFailure::NameMismatch)))
            {
                let Request::TlsConnect { server_name, .. } = &mut event.request else {
                    panic!()
                };
                *server_name = "localhost".into();
            }
        }
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                replay_interpreter(&conflicting, &tree, &tree, io.handles().observed(), b"");
            }))
            .is_err()
        );
    }
}

#[test]
fn fault_inputs_come_from_the_original_fixture_not_the_comparison_tape() {
    use crate::support::io::{ReplayFixture, replay_interpreter_with_fixture};
    use hirsel_rt::io_sim::SimIo;
    let tree = Tree::new(BTreeMap::new()).unwrap();
    let tape = vec![
        Event {
            request: Request::Net { io: 1 },
            completion: Completion::Handle(Ok(2)),
            fault: None,
        },
        Event {
            request: Request::Connect {
                net: 2,
                host: "127.0.0.1".into(),
                port: 443,
                timeout: NetTimeout::Unbounded,
            },
            completion: Completion::NetHandle(Err(NetError::Fault(17))),
            fault: None,
        },
    ];
    let mut native = SimIo::new(tree.clone(), tape.clone());
    native.set_net_rules(
        [NetRule {
            address: "127.0.0.1".into(),
            port: 443,
            operation: NetOperation::Connect,
        }]
        .into(),
    );
    for event in &tape {
        assert_eq!(
            native.call(event.request.clone()).unwrap(),
            event.completion
        );
    }
    native.finish().unwrap();
    let replay = |comparison: &[Event]| {
        replay_interpreter_with_fixture(
            comparison,
            ReplayFixture {
                events: &tape,
                pem_roots: &[],
            },
            &tree,
            native.tree(),
            native.handles().observed(),
            b"",
            &BTreeMap::new(),
        )
    };
    replay(&tape);
    let mut altered = tape.clone();
    altered[1].completion = Completion::NetHandle(Err(NetError::Fault(18)));
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| replay(&altered))).is_err());
}
