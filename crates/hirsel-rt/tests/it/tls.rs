//! Ported before the provider from rustls 0.23.46 rustls/tests/api.rs:
//! client_check_server_certificate, client_stream_read, client_close_notify and
//! server_close_notify. Fixtures: test-ca/rsa-2048 at f04804fa (MIT).
//! Hirsel adds the granted-stream and plaintext-tape boundary.
use crate::support::tls_peer::*;
use hirsel_rt::io::*;
use hirsel_rt::io_real::RealIo;
use hirsel_rt::io_sim::SimIo;
use hirsel_rt::net_tls::{WEBPKI_VERSION, pem_digest};
use std::io::Read;

#[test]
fn pem_success_and_truncated_stream_record_plaintext_and_replay_without_a_server() {
    for (truncated, tls12) in [(false, false), (true, false), (false, true), (true, true)] {
        let (port, thread) = server(truncated, tls12);
        let mut real = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        crate::support::host(&mut real);
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
        crate::support::host(&mut sim);
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
        crate::support::host(&mut io);
        io.set_net_rules(rules(port));
        let (net, stream, done) = connect(&mut io, port, name, roots);
        assert_eq!(done, Completion::Protocol(Err(NetError::Tls(failure))));
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
    // rustls reports a peer that sent no certificate where one is required apart from a
    // bad one.
    assert_eq!(
        failure(&E::NoCertificatesPresented),
        TlsFailure::CertificateRequired
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
            host.tls_connect(
                hirsel_rt::net_tls::engine(),
                1,
                "localhost",
                &digest,
                None,
                &[]
            ),
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
    crate::support::host(&mut io);
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
fn cooperative_tls_retains_handshake_and_plaintext_across_readiness() {
    use hirsel_rt::{net_host::CooperativeNet, readiness::Readiness};
    struct Pump {
        io: RealIo,
        ready: Readiness,
        next: u64,
    }
    impl Adapter for Pump {
        fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
            self.io.preflight(request)
        }
        fn tls_roots(&mut self, stream: u64, pem: &[u8]) {
            self.io.tls_roots(stream, pem);
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
        crate::support::host(&mut io);
        io.set_net_rules(rules(port));
        let ready = Readiness::default();
        io.set_net_provider(CooperativeNet::new(ready.clone()));
        exchange(&mut Pump { io, ready, next: 0 }, port, truncated);
        server.join().unwrap();
    }
}
