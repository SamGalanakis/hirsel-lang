//! The TLS peer of the TLS tests and of their interpreter-replay twins in
//! hirsel-pipeline-tests (`rt_tls`), from rustls 0.23.46 rustls/tests/api.rs
//! (fixtures: test-ca/rsa-2048 at f04804fa, MIT, in `tests/it/tls-data`).
use hirsel_rt::io::*;
use hirsel_rt::net_tls::pem_digest;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use std::io::{Read, Write};
use std::sync::Arc;
pub const PEM: &[u8] = include_bytes!("../it/tls-data/ca.cert");

pub fn server(truncated: bool, tls12: bool) -> (u16, std::thread::JoinHandle<()>) {
    let versions = [if tls12 {
        &rustls::version::TLS12
    } else {
        &rustls::version::TLS13
    }];
    let certs = CertificateDer::pem_slice_iter(include_bytes!("../it/tls-data/end.fullchain"))
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_slice(include_bytes!("../it/tls-data/end.key")).unwrap();
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

pub fn rules(port: u16) -> std::collections::BTreeSet<NetRule> {
    [NetRule {
        address: "127.0.0.1".into(),
        port,
        operation: NetOperation::Connect,
    }]
    .into()
}

pub fn connect(
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
    io.tls_roots(stream, PEM);
    let done = io
        .call(Request::TlsConnect {
            net,
            stream,
            server_name: name.into(),
            roots,
            identity: None,
            alpn: vec![],
        })
        .unwrap();
    (net, stream, done)
}

pub fn exchange(io: &mut impl Adapter, port: u16, truncated: bool) {
    let (net, stream, done) = connect(io, port, "localhost", pem_digest(PEM));
    assert_eq!(done, Completion::Protocol(Ok(None)));
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
