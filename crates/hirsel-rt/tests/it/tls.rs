//! Ported before the provider from rustls 0.23.46 rustls/tests/api.rs:
//! client_check_server_certificate, client_stream_read, client_close_notify and
//! server_close_notify. Fixtures: test-ca/rsa-2048 at f04804fa (MIT).
//! Hirsel adds the granted-stream and plaintext-tape boundary.
use hirsel_rt::io::*;
use hirsel_rt::io_real::RealIo;
use hirsel_rt::io_sim::SimIo;
use hirsel_rt::net_tls::{WEBPKI_VERSION, pem_digest};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use std::io::{Read, Write};
use std::sync::Arc;

const PEM: &[u8] = include_bytes!("tls-data/ca.cert");

fn server(truncated: bool, tls12: bool) -> (u16, std::thread::JoinHandle<()>) {
    let versions = [if tls12 {
        &rustls::version::TLS12
    } else {
        &rustls::version::TLS13
    }];
    let certs = CertificateDer::pem_slice_iter(include_bytes!("tls-data/end.fullchain"))
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_slice(include_bytes!("tls-data/end.key")).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&versions)
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let thread = std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let conn = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut tls = rustls::StreamOwned::new(conn, socket);
        let mut ping = [0; 4];
        // Certificate-negative clients abort their handshake here.
        if tls.read_exact(&mut ping).is_err() {
            return;
        }
        assert_eq!(&ping, b"ping");
        tls.write_all(b"world").unwrap();
        tls.flush().unwrap();
        if !truncated {
            tls.conn.send_close_notify();
            tls.flush().unwrap();
            let mut end = [0];
            assert_eq!(tls.read(&mut end).unwrap(), 0);
        }
    });
    (port, thread)
}

fn rules(port: u16) -> std::collections::BTreeSet<NetRule> {
    [NetRule {
        address: "127.0.0.1".into(),
        port,
        operation: NetOperation::Connect,
    }]
    .into()
}
fn connect(
    io: &mut impl Adapter,
    port: u16,
    name: &str,
    roots: RootsDigest,
) -> (u64, u64, Completion) {
    let Ok(Completion::Handle(Ok(net))) = io.call(Request::Net { io: 1 }) else {
        panic!("net")
    };
    let Ok(Completion::NetHandle(Ok(stream))) = io.call(Request::ConnectAddr {
        net,
        addr: SocketAddr {
            ip: std::net::Ipv4Addr::LOCALHOST.into(),
            port,
        },
        timeout: NetTimeout::Nanos(5_000_000_000),
    }) else {
        panic!("connect")
    };
    for which in [TimeoutKind::Read, TimeoutKind::Write] {
        assert_eq!(
            io.call(Request::SetTimeout {
                net,
                stream,
                which,
                timeout: NetTimeout::Nanos(5_000_000_000)
            }),
            Ok(Completion::NetUnit(Ok(())))
        );
    }
    let done = io
        .call(Request::TlsConnect {
            net,
            stream,
            server_name: name.into(),
            roots,
        })
        .unwrap();
    (net, stream, done)
}
fn exchange(io: &mut impl Adapter, port: u16, truncated: bool) {
    io.tls_roots(PEM);
    let (net, stream, done) = connect(io, port, "localhost", pem_digest(PEM));
    assert_eq!(done, Completion::NetUnit(Ok(())));
    assert_eq!(
        io.call(Request::NetWriteAll {
            net,
            stream,
            bytes: b"ping".to_vec()
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    let mut received = vec![];
    while received.len() < 5 {
        let Ok(Completion::NetBytes(Ok(bytes))) = io.call(Request::NetRead {
            net,
            stream,
            max: 3,
        }) else {
            panic!("read")
        };
        assert!(!bytes.is_empty());
        received.extend(bytes);
    }
    assert_eq!(received, b"world");
    assert_eq!(
        io.call(Request::NetRead {
            net,
            stream,
            max: 3
        }),
        Ok(Completion::NetBytes(if truncated {
            Err(NetError::Tls(TlsFailure::Truncated))
        } else {
            Ok(vec![])
        }))
    );
    if truncated {
        assert_eq!(
            io.call(Request::Close { handle: stream }),
            Ok(Completion::Unit(Ok(())))
        );
    } else {
        assert_eq!(
            io.call(Request::TlsClose { net, stream }),
            Ok(Completion::NetUnit(Ok(())))
        );
    }
}

#[test]
fn pem_success_and_truncated_stream_record_plaintext_and_replay_without_a_server() {
    for (truncated, tls12) in [(false, false), (true, false), (false, true), (true, true)] {
        let (port, thread) = server(truncated, tls12);
        let mut real = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        real.set_net_rules(rules(port));
        exchange(&mut real, port, truncated);
        thread.join().unwrap();
        let tape = real.tape().to_vec();
        assert!(
            tape.iter().any(
                |e| matches!(&e.request, Request::NetWriteAll { bytes, .. } if bytes == b"ping")
            )
        );
        let text = hirsel_rt::launch::render_tape(&tape).unwrap();
        let tape = hirsel_rt::launch::parse_tape(&text).unwrap();
        let tree = Tree::new(Default::default()).unwrap();
        let mut sim = SimIo::new(tree, tape);
        sim.set_net_rules(rules(port));
        exchange(&mut sim, port, truncated);
        assert_eq!(sim.finish(), Ok(()));
    }
}

#[test]
fn webpki_unknown_issuer_and_pem_name_mismatch_consume_the_stream() {
    for (name, roots, failure) in [
        (
            "localhost",
            RootsDigest::WebPki(WEBPKI_VERSION.into()),
            TlsFailure::UnknownIssuer,
        ),
        ("wrong.example", pem_digest(PEM), TlsFailure::NameMismatch),
    ] {
        let (port, thread) = server(false, false);
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        io.set_net_rules(rules(port));
        io.tls_roots(PEM);
        let (net, stream, done) = connect(&mut io, port, name, roots);
        assert_eq!(done, Completion::NetUnit(Err(NetError::Tls(failure))));
        assert_eq!(
            io.call(Request::NetRead {
                net,
                stream,
                max: 1
            }),
            Err(AdapterError::InvalidHandle(stream))
        );
        thread.join().unwrap();
    }
}

/// Replay a host tape in the interpreter from scripted peers derived from
/// `fixture` alone, comparing it with `tape`.
fn replay(tape: &[Event], fixture: &RealIo) {
    use crate::support::io::{ReplayFixture, replay_interpreter_with_fixture};
    let tree = Tree::new(Default::default()).unwrap();
    replay_interpreter_with_fixture(
        tape,
        ReplayFixture {
            events: fixture.tape(),
            pem_roots: &[PEM],
        },
        &tree,
        &tree,
        fixture.handles().observed(),
        b"",
        &Default::default(),
    );
}
fn refuses(tape: &[Event], fixture: &RealIo) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| replay(tape, fixture))).is_err()
}

#[test]
fn a_host_unknown_issuer_tape_replays_independently_in_the_interpreter() {
    let (port, thread) = server(false, false);
    let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    io.set_net_rules(rules(port));
    let Ok(Completion::Handle(Ok(net))) = io.call(Request::Net { io: 1 }) else {
        panic!("net")
    };
    let Ok(Completion::NetHandle(Ok(stream))) = io.call(Request::ConnectAddr {
        net,
        addr: SocketAddr {
            ip: std::net::Ipv4Addr::LOCALHOST.into(),
            port,
        },
        timeout: NetTimeout::Nanos(5_000_000_000),
    }) else {
        panic!("connect")
    };
    assert_eq!((net, stream), (2, 3));
    assert_eq!(
        io.call(Request::TlsConnect {
            net,
            stream,
            server_name: "localhost".into(),
            roots: RootsDigest::WebPki(WEBPKI_VERSION.into()),
        }),
        Ok(Completion::NetUnit(Err(NetError::Tls(
            TlsFailure::UnknownIssuer
        ))))
    );
    thread.join().unwrap();
    replay(io.tape(), &io);
    // Altered observations refuse against the original fixture.
    for altered in [
        TlsFailure::Expired,
        TlsFailure::NameMismatch,
        TlsFailure::AlertReceived(48),
    ] {
        let mut tape = io.tape().to_vec();
        tape.last_mut().unwrap().completion = Completion::NetUnit(Err(NetError::Tls(altered)));
        assert!(refuses(&tape, &io), "{altered:?}");
    }
}

#[test]
fn host_success_and_truncated_tapes_replay_independently_in_the_interpreter() {
    for (truncated, tls12) in [(false, false), (true, false), (false, true), (true, true)] {
        let (port, thread) = server(truncated, tls12);
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        io.set_net_rules(rules(port));
        exchange(&mut io, port, truncated);
        thread.join().unwrap();
        replay(io.tape(), &io);
        // One altered byte of the delivered plaintext refuses.
        let mut tape = io.tape().to_vec();
        let read = tape
            .iter_mut()
            .find_map(|e| match &mut e.completion {
                Completion::NetBytes(Ok(bytes)) if !bytes.is_empty() => Some(bytes),
                _ => None,
            })
            .unwrap();
        read[0] ^= 1;
        assert!(refuses(&tape, &io), "truncated {truncated}");
        // A truncation recorded as a clean end of stream refuses, and vice versa.
        let mut tape = io.tape().to_vec();
        let end = tape
            .iter_mut()
            .filter(|e| matches!(e.request, Request::NetRead { .. }))
            .last()
            .unwrap();
        end.completion = Completion::NetBytes(if truncated {
            Ok(vec![])
        } else {
            Err(NetError::Tls(TlsFailure::Truncated))
        });
        assert!(refuses(&tape, &io), "truncated {truncated}");
    }
}

#[test]
fn certificate_alert_and_protocol_failures_keep_their_public_variants() {
    use hirsel_rt::net_tls::failure;
    use rustls::{CertificateError as C, Error as E};
    for (cert, expected) in [
        (C::UnknownIssuer, TlsFailure::UnknownIssuer),
        (C::Expired, TlsFailure::Expired),
        (C::NotValidYet, TlsFailure::NotYetValid),
        (C::NotValidForName, TlsFailure::NameMismatch),
        (C::Revoked, TlsFailure::Revoked),
        (C::BadEncoding, TlsFailure::BadCertificate),
        (C::BadSignature, TlsFailure::BadCertificate),
    ] {
        assert_eq!(failure(&E::InvalidCertificate(cert)), expected);
    }
    assert_eq!(
        failure(&E::AlertReceived(
            rustls::AlertDescription::HandshakeFailure
        )),
        TlsFailure::AlertReceived(40)
    );
    assert_eq!(
        failure(&E::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered
        )),
        TlsFailure::HandshakeFailure
    );
    assert_eq!(
        failure(&E::PeerMisbehaved(
            rustls::PeerMisbehaved::TooMuchEarlyDataReceived
        )),
        TlsFailure::PeerMisbehaved
    );
}

#[test]
fn pem_is_bound_to_its_digest_and_invalid_bundles_never_start_tls() {
    use hirsel_rt::net::NetProvider;
    for (material, digest) in [
        (PEM, pem_digest(b"a different bundle")),
        (
            b"not a PEM certificate".as_slice(),
            pem_digest(b"not a PEM certificate"),
        ),
        (b"".as_slice(), pem_digest(b"")),
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut host = hirsel_rt::net_host::HostNet::new();
        let addr = SocketAddr {
            ip: std::net::Ipv4Addr::LOCALHOST.into(),
            port: listener.local_addr().unwrap().port(),
        };
        assert_eq!(
            host.connect_addr(1, addr, NetTimeout::Nanos(5_000_000_000)),
            Ok(Ok(()))
        );
        let (mut socket, _) = listener.accept().unwrap();
        host.tls_roots(material);
        assert_eq!(
            host.tls_connect(1, "localhost", &digest),
            Ok(Err(NetError::InvalidInput))
        );
        host.close(1);
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    }
}

#[test]
fn tls_close_on_a_plain_host_stream_succeeds_without_sending_bytes() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    io.set_net_rules(rules(port));
    let Ok(Completion::Handle(Ok(net))) = io.call(Request::Net { io: 1 }) else {
        panic!()
    };
    let Ok(Completion::NetHandle(Ok(stream))) = io.call(Request::ConnectAddr {
        net,
        addr: SocketAddr {
            ip: std::net::Ipv4Addr::LOCALHOST.into(),
            port,
        },
        timeout: NetTimeout::Nanos(5_000_000_000),
    }) else {
        panic!()
    };
    let (mut socket, _) = listener.accept().unwrap();
    assert_eq!(
        io.call(Request::TlsClose { net, stream }),
        Ok(Completion::NetUnit(Ok(())))
    );
    let mut byte = [0];
    assert_eq!(socket.read(&mut byte).unwrap(), 0);
}

#[test]
fn tls_abi_uses_the_core_roots_and_result_layouts() {
    use hirsel_rt::tls_abi::TlsRootsAbi;
    use std::mem::{align_of, offset_of, size_of};
    assert_eq!(
        (size_of::<TlsRootsAbi>(), align_of::<TlsRootsAbi>()),
        (32, 8)
    );
    assert_eq!(offset_of!(TlsRootsAbi, pem), 8);
    assert_eq!(
        (
            size_of::<hirsel_rt::net_abi::NetWordResult>(),
            align_of::<hirsel_rt::net_abi::NetWordResult>()
        ),
        (24, 8)
    );
    assert_eq!(
        (
            size_of::<hirsel_rt::net_abi::NetUnitResult>(),
            align_of::<hirsel_rt::net_abi::NetUnitResult>()
        ),
        (16, 4)
    );
}

#[test]
fn cooperative_tls_retains_handshake_and_plaintext_across_readiness() {
    use hirsel_rt::{net_host::CooperativeNet, readiness::Readiness};
    struct Pump {
        io: RealIo,
        ready: Readiness,
        next: u64,
    }
    impl Adapter for Pump {
        fn tls_roots(&mut self, pem: &[u8]) {
            self.io.tls_roots(pem);
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            self.next += 1;
            self.ready.enter(self.next);
            loop {
                let result = self.io.call(request.clone());
                if result == Err(AdapterError::Blocked) {
                    assert!(self.ready.wait().unwrap());
                } else {
                    self.ready.finish(self.next);
                    return result;
                }
            }
        }
    }
    for (truncated, tls12) in [(false, false), (true, false), (false, true), (true, true)] {
        let (port, server) = server(truncated, tls12);
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        io.set_net_rules(rules(port));
        let ready = Readiness::default();
        io.set_net_provider(CooperativeNet::new(ready.clone()));
        exchange(&mut Pump { io, ready, next: 0 }, port, truncated);
        server.join().unwrap();
    }
}
