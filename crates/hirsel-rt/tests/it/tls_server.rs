//! Server handshakes (`tls.accept`) on the host provider, against rustls 0.23.46 clients
//! as in rustls/tests/api.rs (server_close_notify, server_alpn): the plaintext tape,
//! its replay without a peer, ALPN and identities that never start TLS.
use hirsel_rt::io::*;
use hirsel_rt::io_real::RealIo;
use hirsel_rt::io_sim::SimIo;
use hirsel_rt::net_tls::chain_digest;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use std::io::{Read, Write};
use std::sync::Arc;

const CHAIN: &[u8] = include_bytes!("tls-data/server.cert");
const KEY: &[u8] = include_bytes!("tls-data/server.key");
const OTHER_KEY: &[u8] = include_bytes!("tls-data/other.key");

fn rules() -> std::collections::BTreeSet<NetRule> {
    [NetRule {
        address: "127.0.0.1".into(),
        port: 0,
        operation: NetOperation::ListenTls,
    }]
    .into()
}

/// A verifying client trusting only the self-signed identity; it offers `alpn`.
fn client(port: u16, alpn: &[&[u8]], tls12: bool) -> std::thread::JoinHandle<Option<Vec<u8>>> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(CHAIN).unwrap())
        .unwrap();
    let versions = [if tls12 {
        &rustls::version::TLS12
    } else {
        &rustls::version::TLS13
    }];
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&versions)
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    std::thread::spawn(move || {
        let socket = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let name = ServerName::try_from("localhost").unwrap();
        let conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
        let mut tls = rustls::StreamOwned::new(conn, socket);
        // A refused handshake surfaces at the first write or read.
        tls.write_all(b"ping").ok()?;
        tls.flush().ok()?;
        let mut world = [0; 5];
        tls.read_exact(&mut world).ok()?;
        assert_eq!(&world, b"world");
        let mut end = [0];
        assert_eq!(
            tls.read(&mut end).unwrap(),
            0,
            "close_notify ends the stream"
        );
        Some(tls.conn.alpn_protocol().unwrap_or_default().to_vec())
    })
}

/// Listen, accept one client and present `key` with the test chain. The returned port
/// is the listener's; a host run starts its client from it.
fn accept(io: &mut impl Adapter, key: &[u8], start: impl FnOnce(u16)) -> (u64, u64, Completion) {
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
    io.tls_identity(stream, CHAIN, key);
    let done = io
        .call(Request::TlsAccept {
            net,
            stream,
            identity: chain_digest(CHAIN),
        })
        .unwrap();
    (net, stream, done)
}

fn serve(io: &mut impl Adapter, start: impl FnOnce(u16)) {
    let (net, stream, done) = accept(io, KEY, start);
    assert_eq!(done, Completion::NetUnit(Ok(())));
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
    assert_eq!(
        io.call(Request::TlsClose { net, stream }),
        Ok(Completion::NetUnit(Ok(())))
    );
}

#[test]
fn server_handshake_records_plaintext_and_the_chain_digest_and_replays_without_a_client() {
    let key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
    for tls12 in [false, true] {
        let mut real = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
        real.set_net_rules(rules());
        let mut peer = None;
        serve(&mut real, |port| {
            peer = Some(client(port, &[b"h2", b"http/1.1"], tls12))
        });
        // The server offers only http/1.1, which the client then negotiated.
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
        sim.set_net_rules(rules());
        serve(&mut sim, |_| {});
        assert_eq!(sim.finish(), Ok(()));
    }
}

#[test]
fn a_client_offering_no_shared_alpn_protocol_fails_the_server_handshake() {
    let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    io.set_net_rules(rules());
    let mut peer = None;
    let (net, stream, done) = accept(&mut io, KEY, |port| {
        peer = Some(client(port, &[b"h2"], false))
    });
    assert_eq!(
        done,
        Completion::NetUnit(Err(NetError::Tls(TlsFailure::HandshakeFailure)))
    );
    assert_eq!(peer.unwrap().join().unwrap(), None);
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
        io.set_net_rules(rules());
        let mut socket = None;
        let (_, _, done) = accept(&mut io, key, |port| {
            socket = Some(std::net::TcpStream::connect(("127.0.0.1", port)).unwrap())
        });
        assert_eq!(done, Completion::NetUnit(Err(NetError::InvalidInput)));
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
