//! Server handshakes (`tls.accept`) on the host provider, against rustls 0.23.46 clients
//! as in rustls/tests/api.rs (server_close_notify, server_alpn, client_auth, sni): the
//! plaintext tape, its replay without a peer, ALPN, identities that never start TLS, SNI
//! selection and client certificates, and `tls.connect_as` against a rustls server.
use hirsel_rt::io::*;
use hirsel_rt::io_real::RealIo;
use hirsel_rt::io_sim::SimIo;
use hirsel_rt::net::{ClientAuth, TlsServerDigest};
use hirsel_rt::net_tls::chain_digest;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use std::io::{Read, Write};
use std::sync::Arc;

const CHAIN: &[u8] = include_bytes!("tls-data/server.cert");
const KEY: &[u8] = include_bytes!("tls-data/server.key");
const OTHER_KEY: &[u8] = include_bytes!("tls-data/other.key");
const CLIENT_CA: &[u8] = include_bytes!("tls-data/client-ca.cert");
const ROGUE_CA: &[u8] = include_bytes!("tls-data/rogue-ca.cert");
const CLIENT: &[u8] = include_bytes!("tls-data/client.cert");
const CLIENT_KEY: &[u8] = include_bytes!("tls-data/client.key");
const ALPHA: (&[u8], &[u8]) = (
    include_bytes!("tls-data/alpha.cert"),
    include_bytes!("tls-data/alpha.key"),
);
const BETA: (&[u8], &[u8]) = (
    include_bytes!("tls-data/beta.cert"),
    include_bytes!("tls-data/beta.key"),
);

fn rules() -> std::collections::BTreeSet<NetRule> {
    [NetRule {
        address: "127.0.0.1".into(),
        port: 0,
        operation: NetOperation::ListenTls,
    }]
    .into()
}

/// A verifying rustls client: it trusts `trust` alone, sends `name` as SNI and verification
/// name, offers `alpn` and presents `identity` when asked. It returns the negotiated ALPN
/// protocol, or the error that ended its handshake.
struct Client {
    trust: &'static [u8],
    name: &'static str,
    alpn: &'static [&'static [u8]],
    tls12: bool,
    identity: Option<(&'static [u8], &'static [u8])>,
}
const CLIENT_OF_CHAIN: Client = Client {
    trust: CHAIN,
    name: "localhost",
    alpn: &[b"h2", b"http/1.1"],
    tls12: false,
    identity: None,
};
fn client(port: u16, c: Client) -> std::thread::JoinHandle<Result<Vec<u8>, String>> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(c.trust).unwrap())
        .unwrap();
    let versions = [if c.tls12 {
        &rustls::version::TLS12
    } else {
        &rustls::version::TLS13
    }];
    let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&versions)
    .unwrap()
    .with_root_certificates(roots);
    let mut config = match c.identity {
        Some((chain, key)) => builder
            .with_client_auth_cert(
                vec![CertificateDer::from_pem_slice(chain).unwrap()],
                PrivateKeyDer::from_pem_slice(key).unwrap(),
            )
            .unwrap(),
        None => builder.with_no_client_auth(),
    };
    config.alpn_protocols = c.alpn.iter().map(|p| p.to_vec()).collect();
    std::thread::spawn(move || {
        let socket = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let name = ServerName::try_from(c.name).unwrap();
        let conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
        let mut tls = rustls::StreamOwned::new(conn, socket);
        // A refused handshake surfaces at the first write or read.
        let error = |e: std::io::Error| e.to_string();
        tls.write_all(b"ping").map_err(error)?;
        tls.flush().map_err(error)?;
        let mut world = [0; 5];
        tls.read_exact(&mut world).map_err(error)?;
        assert_eq!(&world, b"world");
        let mut end = [0];
        assert_eq!(
            tls.read(&mut end).unwrap(),
            0,
            "close_notify ends the stream"
        );
        Ok(tls.conn.alpn_protocol().unwrap_or_default().to_vec())
    })
}

/// The ALPN list these servers select from, as an HTTP/1.1 server's.
fn http1() -> Vec<String> {
    vec!["http/1.1".into()]
}

/// A server of the test chain and `key` that asks for no client certificate.
fn single(
    key: &'static [u8],
) -> (
    Vec<(&'static [u8], &'static [u8])>,
    ClientAuth<&'static [u8]>,
) {
    (vec![(CHAIN, key)], ClientAuth::Anonymous)
}

/// Listen, accept one client and handshake as `server`. The returned port is the
/// listener's; a host run starts its client from it.
fn accept(
    io: &mut impl Adapter,
    server: (Vec<(&[u8], &[u8])>, ClientAuth<&[u8]>),
    start: impl FnOnce(u16),
) -> (u64, u64, Completion) {
    let Ok(Completion::Handle(Ok(net))) = io.call(Request::Net { io: 1 }) else {
        panic!("net")
    };
    let Ok(Completion::Listened(Ok((listener, local)))) = io.call(Request::Listen {
        net,
        addr: SocketAddr {
            ip: std::net::Ipv4Addr::LOCALHOST.into(),
            port: 0,
        },
    }) else {
        panic!("listen")
    };
    start(local.port);
    let Ok(Completion::Accepted(Ok((stream, _)))) = io.call(Request::Accept { net, listener })
    else {
        panic!("accept")
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
    // A ListenTls stream carries no plaintext before its handshake.
    assert_eq!(
        io.call(Request::NetRead {
            net,
            stream,
            max: 1
        }),
        Ok(Completion::NetBytes(Err(NetError::NotGranted)))
    );
    let (identities, clients) = server;
    io.tls_server(stream, &identities, clients, &http1());
    let done = io
        .call(Request::TlsAccept {
            net,
            stream,
            server: TlsServerDigest::of(&identities, clients, &http1()),
        })
        .unwrap();
    (net, stream, done)
}

/// Handshake as `server`, exchange ping and world, and return what `tls.peer` answered.
fn serve(
    io: &mut impl Adapter,
    server: (Vec<(&[u8], &[u8])>, ClientAuth<&[u8]>),
    start: impl FnOnce(u16),
) -> Option<Vec<u8>> {
    let (net, stream, done) = accept(io, server, start);
    assert!(matches!(done, Completion::Protocol(Ok(_))), "{done:?}");
    let mut received = vec![];
    while received.len() < 4 {
        let Ok(Completion::NetBytes(Ok(bytes))) = io.call(Request::NetRead {
            net,
            stream,
            max: 4,
        }) else {
            panic!("read")
        };
        assert!(!bytes.is_empty());
        received.extend(bytes);
    }
    assert_eq!(received, b"ping");
    assert_eq!(
        io.call(Request::NetWriteAll {
            net,
            stream,
            bytes: b"world".to_vec()
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    let Ok(Completion::Certificates(Ok(peer))) = io.call(Request::TlsPeer { net, stream }) else {
        panic!("tls.peer")
    };
    assert_eq!(
        io.call(Request::TlsClose { net, stream }),
        Ok(Completion::NetUnit(Ok(())))
    );
    peer
}

#[test]
fn server_handshake_records_plaintext_and_the_chain_digest_and_replays_without_a_client() {
    let key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
    for tls12 in [false, true] {
        let mut real = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        crate::support::host(&mut real);
        real.set_net_rules(rules());
        let mut peer = None;
        let anonymous = serve(&mut real, single(KEY), |port| {
            peer = Some(client(
                port,
                Client {
                    tls12,
                    ..CLIENT_OF_CHAIN
                },
            ))
        });
        assert_eq!(anonymous, None);
        // The server selects only http/1.1, which the client then negotiated.
        assert_eq!(peer.unwrap().join().unwrap().unwrap(), b"http/1.1");
        let tape = real.tape().to_vec();
        let text = hirsel_rt::launch::render_tape(&tape).unwrap();
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert!(text.contains(&format!("pem:{}", hex(&chain_digest(CHAIN)))));
        // Neither the key nor any digest of it reaches the tape.
        for secret in [KEY, key.secret_der()] {
            assert!(!text.contains(&hex(secret)));
            assert!(!text.contains(&hex(&chain_digest(secret))));
        }
        let mut sim = SimIo::new(
            Tree::new(Default::default()).unwrap(),
            hirsel_rt::launch::parse_tape(&text).unwrap(),
        );
        crate::support::host(&mut sim);
        sim.set_net_rules(rules());
        serve(&mut sim, single(KEY), |_| {});
        assert_eq!(sim.finish(), Ok(()));
    }
}

#[test]
fn a_client_offering_no_shared_alpn_protocol_fails_the_server_handshake() {
    let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    crate::support::host(&mut io);
    io.set_net_rules(rules());
    let mut peer = None;
    let (net, stream, done) = accept(&mut io, single(KEY), |port| {
        peer = Some(client(
            port,
            Client {
                alpn: &[b"h2"],
                ..CLIENT_OF_CHAIN
            },
        ))
    });
    assert_eq!(
        done,
        Completion::Protocol(Err(NetError::Tls(TlsFailure::HandshakeFailure)))
    );
    assert!(peer.unwrap().join().unwrap().is_err());
    // The failed handshake consumed the stream.
    assert_eq!(
        io.call(Request::NetRead {
            net,
            stream,
            max: 1
        }),
        Err(AdapterError::InvalidHandle(stream))
    );
}

#[test]
fn an_invalid_identity_is_invalid_input_and_never_starts_tls() {
    for key in [OTHER_KEY, b"".as_slice(), CHAIN] {
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        crate::support::host(&mut io);
        io.set_net_rules(rules());
        let mut socket = None;
        let (_, _, done) = accept(&mut io, single(key), |port| {
            socket = Some(std::net::TcpStream::connect(("127.0.0.1", port)).unwrap())
        });
        assert_eq!(done, Completion::Protocol(Err(NetError::InvalidInput)));
        // The stream closed without one TLS byte.
        let mut byte = [0];
        assert_eq!(socket.unwrap().read(&mut byte).unwrap(), 0);
    }
}

#[test]
fn identity_abi_is_two_core_byte_slices() {
    use hirsel_rt::tls_abi::TlsIdentityAbi;
    use std::mem::{align_of, offset_of, size_of};
    assert_eq!(
        (size_of::<TlsIdentityAbi>(), align_of::<TlsIdentityAbi>()),
        (48, 8)
    );
    assert_eq!(offset_of!(TlsIdentityAbi, key), 24);
}

/// The DER of a PEM certificate, as `tls.peer` returns a one-certificate chain.
fn der(pem: &[u8]) -> Vec<u8> {
    CertificateDer::from_pem_slice(pem).unwrap().to_vec()
}

#[test]
fn a_required_client_certificate_is_verified_returned_by_tls_peer_and_replayed() {
    let mut real = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    crate::support::host(&mut real);
    real.set_net_rules(rules());
    let server = || (vec![(CHAIN, KEY)], ClientAuth::Require(CLIENT_CA));
    let mut peer = None;
    let chain = serve(&mut real, server(), |port| {
        peer = Some(client(
            port,
            Client {
                identity: Some((CLIENT, CLIENT_KEY)),
                ..CLIENT_OF_CHAIN
            },
        ))
    });
    assert!(peer.unwrap().join().unwrap().is_ok());
    assert_eq!(chain, Some(der(CLIENT)));
    // The tape names the trust set by digest and carries the public chain tls.peer returned.
    let text = hirsel_rt::launch::render_tape(real.tape()).unwrap();
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    assert!(text.contains(&format!("require:{}", hex(&chain_digest(CLIENT_CA)))));
    assert!(text.contains(&format!("der:{}", hex(&der(CLIENT)))));
    let mut sim = SimIo::new(
        Tree::new(Default::default()).unwrap(),
        hirsel_rt::launch::parse_tape(&text).unwrap(),
    );
    crate::support::host(&mut sim);
    sim.set_net_rules(rules());
    assert_eq!(serve(&mut sim, server(), |_| {}), Some(der(CLIENT)));
    assert_eq!(sim.finish(), Ok(()));
}

#[test]
fn a_missing_or_untrusted_client_certificate_is_refused() {
    for (clients, identity, expected) in [
        (
            ClientAuth::Require(CLIENT_CA),
            None,
            TlsFailure::CertificateRequired,
        ),
        (
            ClientAuth::Require(ROGUE_CA),
            Some((CLIENT, CLIENT_KEY)),
            TlsFailure::UnknownIssuer,
        ),
        (
            ClientAuth::Request(ROGUE_CA),
            Some((CLIENT, CLIENT_KEY)),
            TlsFailure::UnknownIssuer,
        ),
    ] {
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        crate::support::host(&mut io);
        io.set_net_rules(rules());
        let mut peer = None;
        let (net, stream, done) = accept(&mut io, (vec![(CHAIN, KEY)], clients), |port| {
            peer = Some(client(
                port,
                Client {
                    identity,
                    ..CLIENT_OF_CHAIN
                },
            ))
        });
        assert_eq!(done, Completion::Protocol(Err(NetError::Tls(expected))));
        // The client learns why: certificate_required or unknown_ca.
        let refused = peer.unwrap().join().unwrap().unwrap_err();
        assert!(
            refused.contains("CertificateRequired") || refused.contains("UnknownCA"),
            "{refused}"
        );
        assert_eq!(
            io.call(Request::TlsPeer { net, stream }),
            Err(AdapterError::InvalidHandle(stream))
        );
    }
}

#[test]
fn a_requested_certificate_is_optional_and_anonymous_servers_never_see_one() {
    for (clients, identity) in [
        (ClientAuth::Request(CLIENT_CA), None),
        (ClientAuth::Anonymous, Some((CLIENT, CLIENT_KEY))),
    ] {
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        crate::support::host(&mut io);
        io.set_net_rules(rules());
        let mut peer = None;
        let chain = serve(&mut io, (vec![(CHAIN, KEY)], clients), |port| {
            peer = Some(client(
                port,
                Client {
                    identity,
                    ..CLIENT_OF_CHAIN
                },
            ))
        });
        assert!(peer.unwrap().join().unwrap().is_ok());
        assert_eq!(chain, None);
    }
}

#[test]
fn sni_selects_the_identity_naming_it_and_otherwise_the_first() {
    for (name, trust) in [
        ("beta.hirsel.test", BETA.0),
        ("alpha.hirsel.test", ALPHA.0),
        // No identity names it: the first is presented, for a client trusting it by name.
        ("localhost", CHAIN),
    ] {
        for order in [vec![ALPHA, BETA], vec![BETA, ALPHA]] {
            let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
            crate::support::host(&mut io);
            io.set_net_rules(rules());
            let first = order[0];
            let identities = if name == "localhost" {
                vec![(CHAIN, KEY), first]
            } else {
                order
            };
            let mut peer = None;
            serve(&mut io, (identities, ClientAuth::Anonymous), |port| {
                peer = Some(client(
                    port,
                    Client {
                        trust,
                        name,
                        ..CLIENT_OF_CHAIN
                    },
                ))
            });
            assert!(peer.unwrap().join().unwrap().is_ok(), "{name}");
        }
    }
}

#[test]
fn an_empty_identity_list_or_trust_set_is_invalid_input() {
    for server in [
        (vec![], ClientAuth::Anonymous),
        (vec![(CHAIN, KEY)], ClientAuth::Require(b"".as_slice())),
        (vec![(CHAIN, KEY)], ClientAuth::Request(KEY)),
    ] {
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        crate::support::host(&mut io);
        io.set_net_rules(rules());
        let mut socket = None;
        let (_, _, done) = accept(&mut io, server, |port| {
            socket = Some(std::net::TcpStream::connect(("127.0.0.1", port)).unwrap())
        });
        assert_eq!(done, Completion::Protocol(Err(NetError::InvalidInput)));
    }
}

/// `tls.connect_as` presents its identity to a rustls server that requires one, as
/// rustls/tests/api.rs client_auth does, and `tls.peer` returns the server's chain.
#[test]
fn connect_as_presents_the_client_identity_to_a_requiring_server() {
    for identity in [Some((CLIENT, CLIENT_KEY)), None] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let verifier = hirsel_run_schema::tls::client_verifier(
            hirsel_run_schema::tls::pem_roots(CLIENT_CA).unwrap(),
            true,
        )
        .unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![CertificateDer::from_pem_slice(ALPHA.0).unwrap()],
            PrivateKeyDer::from_pem_slice(ALPHA.1).unwrap(),
        )
        .unwrap();
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let conn = rustls::ServerConnection::new(Arc::new(config)).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, socket);
            let mut ping = [0; 4];
            tls.read_exact(&mut ping).ok()?;
            tls.conn.peer_certificates().map(|chain| chain[0].to_vec())
        });
        let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        crate::support::host(&mut io);
        io.set_net_rules(
            [NetRule {
                address: "127.0.0.1".into(),
                port,
                operation: NetOperation::Connect,
            }]
            .into(),
        );
        let Ok(Completion::Handle(Ok(net))) = io.call(Request::Net { io: 1 }) else {
            panic!("net")
        };
        let Ok(Completion::NetHandle(Ok(stream))) = io.call(Request::Connect {
            net,
            host: "127.0.0.1".into(),
            port,
            timeout: NetTimeout::Nanos(5_000_000_000),
        }) else {
            panic!("connect")
        };
        io.tls_roots(stream, ALPHA.0);
        if let Some((chain, key)) = identity {
            io.tls_identity(stream, chain, key);
        }
        let connected = io.call(Request::TlsConnect {
            net,
            stream,
            server_name: "alpha.hirsel.test".into(),
            roots: hirsel_rt::net_tls::pem_digest(ALPHA.0),
            identity: identity.map(|(chain, _)| chain_digest(chain)),
            alpn: vec![],
        });
        // TLS 1.3 completes the client's handshake before the server checks its
        // certificate, so a refusal surfaces at the first exchange.
        let written = io.call(Request::NetWriteAll {
            net,
            stream,
            bytes: b"ping".to_vec(),
        });
        let read = io.call(Request::NetRead {
            net,
            stream,
            max: 1,
        });
        let seen = server.join().unwrap();
        // The client's tls.peer is the server chain it verified, as rustls reports it.
        let peer = io.call(Request::TlsPeer { net, stream });
        if identity.is_some() {
            assert_eq!(connected, Ok(Completion::Protocol(Ok(None))));
            assert_eq!(written, Ok(Completion::NetUnit(Ok(()))));
            assert_eq!(seen, Some(der(CLIENT)));
            assert_eq!(peer, Ok(Completion::Certificates(Ok(Some(der(ALPHA.0))))));
        } else {
            // The server refuses with certificate_required at the first exchange.
            assert_eq!(seen, None);
            assert_eq!(connected, Ok(Completion::Protocol(Ok(None))));
            assert!(
                written != Ok(Completion::NetUnit(Ok(())))
                    || read
                        == Ok(Completion::NetBytes(Err(NetError::Tls(
                            TlsFailure::AlertReceived(116)
                        )))),
                "{written:?} {read:?}"
            );
        }
    }
}
