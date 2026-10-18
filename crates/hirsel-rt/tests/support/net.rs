//! Test-only translation from native completion tapes to independent scripted
//! peers. The script preserves accepted writes, read chunk boundaries and EOF;
//! interpreter execution must reproduce each original completion and then replay
//! its own capability tape strictly. A handshake failure other than a name
//! mismatch becomes a `Fails(f)` peer and a truncated TLS read a `Truncate()`
//! step. No production replay protocol is changed.
use hirsel_interp::{
    Integer, Value,
    library::{self, collection},
};
use hirsel_rt::{
    io::{Completion as C, Event, Request as R},
    net::*,
};
use hirsel_run_config::{NetConnection, NetGrant, NetListener, NetPeer, NetStep, PeerTls};
use hirsel_types::{BuiltinType, Primitive};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub fn rules(rules: &BTreeSet<NetRule>) -> Vec<hirsel_run_config::NetRule> {
    rules
        .iter()
        .map(|r| hirsel_run_config::NetRule {
            address: r.address.clone(),
            service: r.port.to_string(),
            operation: r.operation.name().into(),
        })
        .collect()
}
pub fn fixture(tape: &[Event], granted: &BTreeSet<NetRule>) -> Result<NetGrant, String> {
    #[derive(Default)]
    struct TlsIdentity {
        names: BTreeSet<String>,
        mismatches: BTreeSet<String>,
        failures: Vec<TlsFailure>,
    }
    let mut identities = BTreeMap::<usize, TlsIdentity>::new();
    let mut peers: Vec<NetPeer> = vec![];
    let mut streams = BTreeMap::<u64, (usize, usize)>::new();
    fn endpoint(peers: &mut Vec<NetPeer>, host: &str, port: u16) -> usize {
        if let Some(i) = peers
            .iter()
            .position(|p| p.address == host && p.port == port)
        {
            return i;
        }
        peers.push(NetPeer {
            address: host.into(),
            port,
            resolves_to: vec![],
            tls: PeerTls::Plain,
            connections: vec![],
        });
        peers.len() - 1
    }
    fn steps<'a>(
        peers: &'a mut [NetPeer],
        streams: &BTreeMap<u64, (usize, usize)>,
        id: u64,
    ) -> Result<&'a mut Vec<NetStep>, String> {
        let &(p, c) = streams
            .get(&id)
            .ok_or_else(|| format!("no recorded connection for stream {id}"))?;
        let NetConnection::Open(steps) = &mut peers[p].connections[c] else {
            unreachable!()
        };
        Ok(steps)
    }
    for event in tape {
        match (&event.request, &event.completion) {
            (R::Resolve { host, port, .. }, C::Addresses(Ok(addresses))) => {
                if addresses.iter().any(|a| a.port != *port) {
                    return Err("resolved port differs from request".into());
                }
                let p = endpoint(&mut peers, host, *port);
                peers[p].resolves_to = addresses.iter().map(|a| a.ip.to_string()).collect();
            }
            (request @ (R::Connect { .. } | R::ConnectAddr { .. }), C::NetHandle(result)) => {
                let (host, port) = match request {
                    R::Connect { host, port, .. } => (host.clone(), *port),
                    R::ConnectAddr { addr, .. } => (addr.ip.to_string(), addr.port),
                    _ => unreachable!(),
                };
                let connection = match result {
                    Ok(_) => NetConnection::Open(vec![]),
                    Err(NetError::ConnectionRefused) => NetConnection::Refused,
                    Err(NetError::TimedOut) => NetConnection::Unanswered,
                    Err(
                        NetError::NotGranted
                        | NetError::InvalidInput
                        | NetError::Canceled(_)
                        | NetError::Fault(_),
                    ) => continue,
                    Err(e) => return Err(format!("script cannot represent connect outcome {e:?}")),
                };
                let p = endpoint(&mut peers, &host, port);
                let c = peers[p].connections.len();
                peers[p].connections.push(connection);
                if let Ok(id) = result {
                    streams.insert(*id, (p, c));
                }
            }
            (R::NetRead { stream, .. }, C::NetBytes(Ok(bytes))) => {
                steps(&mut peers, &streams, *stream)?.push(if bytes.is_empty() {
                    NetStep::Close
                } else {
                    NetStep::Send(bytes.clone())
                })
            }
            (R::NetWrite { stream, bytes, .. }, C::Count(Ok(count))) => {
                let accepted = bytes
                    .get(..usize::try_from(*count).map_err(|_| "write count overflow")?)
                    .ok_or("write count exceeds input")?;
                steps(&mut peers, &streams, *stream)?.push(NetStep::Expect(accepted.to_vec()));
            }
            (R::NetWriteAll { stream, bytes, .. }, C::NetUnit(Ok(()))) => {
                steps(&mut peers, &streams, *stream)?.push(NetStep::Expect(bytes.clone()))
            }
            (R::NetRead { stream, .. }, C::NetBytes(Err(NetError::TimedOut))) => {
                steps(&mut peers, &streams, *stream)?.push(NetStep::Stall)
            }
            (R::NetRead { stream, .. }, C::NetBytes(Err(NetError::Tls(TlsFailure::Truncated)))) => {
                steps(&mut peers, &streams, *stream)?.push(NetStep::Truncate)
            }
            (R::NetRead { stream, .. }, C::NetBytes(Err(NetError::ConnectionReset)))
            | (R::NetWrite { stream, .. }, C::Count(Err(NetError::ConnectionReset)))
            | (R::NetWriteAll { stream, .. }, C::NetUnit(Err(NetError::ConnectionReset))) => {
                steps(&mut peers, &streams, *stream)?.push(NetStep::Reset)
            }
            (R::PeerAddr { stream, .. }, C::Address(Ok(addr))) => {
                let &(p, _) = streams.get(stream).ok_or("peer address without a stream")?;
                if peers[p].address.parse::<std::net::IpAddr>().is_err()
                    && peers[p].resolves_to.is_empty()
                {
                    peers[p].resolves_to.push(addr.ip.to_string());
                }
            }
            (
                R::TlsConnect {
                    stream,
                    server_name,
                    ..
                },
                C::NetUnit(result),
            ) => {
                let &(p, _) = streams.get(stream).ok_or("TLS without a stream")?;
                let identity = identities.entry(p).or_default();
                match result {
                    Ok(()) => {
                        identity.names.insert(server_name.clone());
                    }
                    Err(NetError::Tls(TlsFailure::NameMismatch)) => {
                        identity.mismatches.insert(server_name.clone());
                    }
                    Err(NetError::Tls(failure)) => {
                        if !identity.failures.contains(failure) {
                            identity.failures.push(*failure);
                        }
                    }
                    Err(NetError::NotGranted | NetError::Canceled(_) | NetError::Fault(_)) => {}
                    Err(e) => return Err(format!("script cannot represent TLS outcome {e:?}")),
                }
            }
            (R::TlsClose { .. }, C::NetUnit(Err(e)))
                if !matches!(
                    e,
                    NetError::NotGranted
                        | NetError::ConnectionReset
                        | NetError::Canceled(_)
                        | NetError::Fault(_)
                ) =>
            {
                return Err(format!("script cannot represent TLS close outcome {e:?}"));
            }
            _ => {}
        }
    }
    for (p, identity) in identities {
        let certificate = !identity.names.is_empty() || !identity.mismatches.is_empty();
        peers[p].tls = match identity.failures[..] {
            [] => {
                if !identity.names.is_disjoint(&identity.mismatches) {
                    return Err(
                        "one peer cannot both accept and reject the same TLS server name".into(),
                    );
                }
                PeerTls::Certificate(identity.names.into_iter().collect())
            }
            // A host failure other than a name mismatch is the peer's, for every name.
            [failure] if !certificate => PeerTls::Fails(failure.into()),
            _ => {
                return Err(
                    "one peer cannot have two TLS identities: it fails every handshake one way"
                        .into(),
                );
            }
        };
    }
    Ok(NetGrant {
        provider: "net.script@2".into(),
        rules: rules(granted),
        peers: Some(peers),
        listeners: vec![],
    })
}
fn handle(kind: &str, id: u64) -> Value {
    Value::Handle {
        kind: kind.into(),
        id,
    }
}
fn number(kind: Primitive, bits: u128) -> Value {
    Value::Integer(Integer::new(kind, bits).unwrap())
}
fn variant(tag: u32, values: Vec<Value>) -> Value {
    Value::Variant {
        variant: tag,
        fields: values.into_iter().map(Some).collect(),
    }
}
fn record(name: &str, values: Vec<Value>) -> Value {
    Value::Record {
        definition: hirsel_syntax::DefPath::core(name),
        fields: values.into_iter().map(Some).collect(),
    }
}
fn list(values: impl IntoIterator<Item = Value>) -> Value {
    collection::sequence(BuiltinType::List, values)
}
fn ip(ip: std::net::IpAddr) -> Value {
    let (tag, octets) = match ip {
        std::net::IpAddr::V4(a) => (0, a.octets().to_vec()),
        std::net::IpAddr::V6(a) => (1, a.octets().to_vec()),
    };
    variant(
        tag,
        vec![collection::sequence(
            BuiltinType::Inline,
            octets.into_iter().map(|b| number(Primitive::U8, b.into())),
        )],
    )
}
fn address(addr: SocketAddr) -> Value {
    record(
        "SocketAddr",
        vec![ip(addr.ip), number(Primitive::U16, addr.port.into())],
    )
}
fn timeout(t: NetTimeout) -> Value {
    match t {
        NetTimeout::Unbounded => variant(0, vec![]),
        NetTimeout::Nanos(n) => variant(1, vec![library::u64_value(n)]),
    }
}
fn tls_failure(f: TlsFailure) -> Value {
    match f {
        TlsFailure::UnknownIssuer => variant(0, vec![]),
        TlsFailure::Expired => variant(1, vec![]),
        TlsFailure::NotYetValid => variant(2, vec![]),
        TlsFailure::NameMismatch => variant(3, vec![]),
        TlsFailure::Revoked => variant(4, vec![]),
        TlsFailure::BadCertificate => variant(5, vec![]),
        TlsFailure::AlertReceived(alert) => variant(6, vec![number(Primitive::U8, alert.into())]),
        TlsFailure::HandshakeFailure => variant(7, vec![]),
        TlsFailure::PeerMisbehaved => variant(8, vec![]),
        TlsFailure::Truncated => variant(9, vec![]),
    }
}
fn rule_value(r: &NetRule) -> Value {
    record(
        "NetRule",
        vec![
            Value::String(r.address.clone()),
            number(Primitive::U16, r.port.into()),
            variant(
                match r.operation {
                    NetOperation::Connect => 0,
                    NetOperation::ConnectTls => 1,
                    NetOperation::Listen => 2,
                    NetOperation::UdpBind => 3,
                    NetOperation::UdpPeer => 4,
                    NetOperation::ListenTls => 5,
                    NetOperation::UnixConnect => 6,
                    NetOperation::UnixListen => 7,
                    NetOperation::UnixListenRemoveStale => 8,
                },
                vec![],
            ),
        ],
    )
}
fn peer_tls(tls: &PeerTls) -> Value {
    match tls {
        PeerTls::Plain => variant(0, vec![]),
        PeerTls::Certificate(names) => {
            variant(1, vec![list(names.iter().cloned().map(Value::String))])
        }
        PeerTls::Fails(failure) => variant(2, vec![tls_failure((*failure).into())]),
    }
}
fn peer(p: &NetPeer) -> Value {
    record(
        "NetPeer",
        vec![
            Value::String(p.address.clone()),
            number(Primitive::U16, p.port.into()),
            list(p.resolves_to.iter().map(|a| ip(a.parse().unwrap()))),
            peer_tls(&p.tls),
            list(p.connections.iter().map(|c| match c {
                NetConnection::Refused => variant(0, vec![]),
                NetConnection::Unanswered => variant(1, vec![]),
                NetConnection::Open(s) => variant(2, vec![steps(s)]),
            })),
        ],
    )
}
fn steps(steps: &[NetStep]) -> Value {
    list(steps.iter().map(|s| match s {
        NetStep::Send(b) => variant(0, vec![Value::Bytes(b.clone())]),
        NetStep::Expect(b) => variant(1, vec![Value::Bytes(b.clone())]),
        NetStep::Stall => variant(2, vec![]),
        NetStep::Close => variant(3, vec![]),
        NetStep::Reset => variant(4, vec![]),
        NetStep::Truncate => variant(5, vec![]),
    }))
}
fn listener(l: &NetListener) -> Value {
    let at = |ip_text: &str, port| {
        address(SocketAddr {
            ip: ip_text.parse().unwrap(),
            port,
        })
    };
    record(
        "NetListener",
        vec![
            Value::String(l.address.clone()),
            number(Primitive::U16, l.port.into()),
            list(l.clients.iter().map(|c| {
                record(
                    "NetClient",
                    vec![at(&c.address, c.port), peer_tls(&c.tls), steps(&c.steps)],
                )
            })),
        ],
    )
}
/// A partial native write is an accepted prefix. The fixture presents exactly
/// that prefix to the script (whose writes always accept their entire input).
pub fn arguments(event: &Event, fixture: super::ReplayFixture<'_>) -> (&'static str, Vec<Value>) {
    let n = |id| handle("Net", id);
    let s = |id| handle("Stream", id);
    match &event.request {
        R::UdpBind { net, addr } => ("net.udp_bind", vec![n(*net), address(*addr)]),
        R::UdpSend {
            net,
            socket,
            bytes,
            addr,
        } => (
            "udp.send_to",
            vec![
                n(*net),
                handle("UdpSocket", *socket),
                Value::Bytes(bytes.clone()),
                address(*addr),
            ],
        ),
        R::UdpRecv { net, socket, max } => (
            "udp.recv_from",
            vec![
                n(*net),
                handle("UdpSocket", *socket),
                list([]),
                number(Primitive::U64, (*max).into()),
            ],
        ),
        R::TestNet {
            test,
            peers,
            listeners,
        } => (
            "test.net",
            vec![
                handle("Test", *test),
                list(peers.iter().map(peer)),
                list(listeners.iter().map(listener)),
            ],
        ),
        R::NetNarrow { net, rules } => (
            "net.narrow",
            vec![n(*net), list(rules.iter().map(rule_value))],
        ),
        R::Resolve { net, host, port } => (
            "net.resolve",
            vec![
                n(*net),
                Value::String(host.clone()),
                number(Primitive::U16, (*port).into()),
            ],
        ),
        R::Connect {
            net,
            host,
            port,
            timeout: t,
        } => (
            "net.connect",
            vec![
                n(*net),
                Value::String(host.clone()),
                number(Primitive::U16, (*port).into()),
                timeout(*t),
            ],
        ),
        R::ConnectAddr {
            net,
            addr,
            timeout: t,
        } => (
            "net.connect_addr",
            vec![n(*net), address(*addr), timeout(*t)],
        ),
        R::NetRead { net, stream, max } => (
            "net.read",
            vec![n(*net), s(*stream), list([]), library::u64_value(*max)],
        ),
        R::NetWrite { net, stream, bytes } => {
            let b = if let C::Count(Ok(count)) = &event.completion {
                bytes
                    .get(..*count as usize)
                    .expect("validated accepted write prefix")
            } else {
                bytes
            };
            (
                "net.write",
                vec![n(*net), s(*stream), Value::Bytes(b.to_vec())],
            )
        }
        R::NetWriteAll { net, stream, bytes } => (
            "net.write_all",
            vec![n(*net), s(*stream), Value::Bytes(bytes.clone())],
        ),
        R::Shutdown { net, stream, how } => (
            "net.shutdown",
            vec![
                n(*net),
                s(*stream),
                variant(
                    match how {
                        ShutdownHow::Read => 0,
                        ShutdownHow::Write => 1,
                        ShutdownHow::Both => 2,
                    },
                    vec![],
                ),
            ],
        ),
        R::SetTimeout {
            net,
            stream,
            which,
            timeout: t,
        } => (
            if *which == TimeoutKind::Read {
                "net.set_read_timeout"
            } else {
                "net.set_write_timeout"
            },
            vec![n(*net), s(*stream), timeout(*t)],
        ),
        R::PeerAddr { net, stream } => ("net.peer_addr", vec![n(*net), s(*stream)]),
        R::TlsConnect {
            net,
            stream,
            server_name,
            roots,
        } => {
            let roots = match roots {
                RootsDigest::WebPki(version) => {
                    assert_eq!(version, "webpki-roots@1.0.9");
                    variant(0, vec![])
                }
                RootsDigest::Pem(digest) => {
                    let pem = fixture
                        .pem_roots
                        .iter()
                        .find(|pem| <[u8; 32]>::from(Sha256::digest(pem)) == *digest)
                        .expect("PEM tape translation requires its original trust bundle");
                    variant(1, vec![Value::Bytes(pem.to_vec())])
                }
            };
            (
                "tls.connect",
                vec![
                    n(*net),
                    s(*stream),
                    Value::String(server_name.clone()),
                    roots,
                ],
            )
        }
        R::TlsAccept {
            net,
            stream,
            identity,
        } => {
            let (chain, key) = fixture
                .tls_identities
                .iter()
                .find(|(chain, _)| <[u8; 32]>::from(Sha256::digest(chain)) == *identity)
                .expect("identity tape translation requires its original chain and key");
            (
                "tls.accept",
                vec![
                    n(*net),
                    s(*stream),
                    record(
                        "TlsIdentity",
                        vec![Value::Bytes(chain.to_vec()), Value::Bytes(key.to_vec())],
                    ),
                ],
            )
        }
        R::TlsClose { net, stream } => ("tls.close", vec![n(*net), s(*stream)]),
        R::Listen { net, addr } => ("net.listen", vec![n(*net), address(*addr)]),
        R::UnixConnect { net, path } => (
            "net.unix_connect",
            vec![n(*net), hirsel_interp::library::path(path)],
        ),
        R::UnixListen { net, path } => (
            "net.unix_listen",
            vec![n(*net), hirsel_interp::library::path(path)],
        ),
        R::Accept { net, listener } => ("net.accept", vec![n(*net), handle("Listener", *listener)]),
        R::LocalAddr { net, listener } => (
            "net.local_addr",
            vec![n(*net), handle("Listener", *listener)],
        ),
        _ => panic!("not a network request"),
    }
}
fn fields(value: Value) -> (u32, Vec<Value>) {
    let Value::Variant { variant, fields } = value else {
        panic!("expected variant")
    };
    (variant, fields.into_iter().map(Option::unwrap).collect())
}
fn uint(value: Value) -> u64 {
    let Value::Integer(n) = value else {
        panic!("expected integer")
    };
    n.bits() as u64
}
fn id(value: Value) -> u64 {
    let Value::Handle { id, .. } = value else {
        panic!("expected handle")
    };
    id
}
fn result(value: Value) -> Result<Value, NetError> {
    let (tag, mut payload) = fields(value);
    let value = payload.remove(0);
    if tag == 0 {
        return Ok(value);
    }
    let (tag, mut payload) = fields(value);
    Err(match tag {
        0 => NetError::NotGranted,
        1 => NetError::Canceled(fields(payload.remove(0)).0),
        2 => NetError::InvalidInput,
        3 => NetError::UnknownHost,
        4 => NetError::ResolverFailure,
        5 => NetError::ConnectionRefused,
        6 => NetError::ConnectionReset,
        7 => NetError::ConnectionAborted,
        8 => NetError::NotConnected,
        9 => NetError::AddrInUse,
        10 => NetError::AddrNotAvailable,
        11 => NetError::HostUnreachable,
        12 => NetError::NetworkUnreachable,
        13 => NetError::NetworkDown,
        14 => NetError::BrokenPipe,
        15 => NetError::TimedOut,
        16 => {
            let (tag, mut p) = fields(payload.remove(0));
            NetError::Tls(match tag {
                0 => TlsFailure::UnknownIssuer,
                1 => TlsFailure::Expired,
                2 => TlsFailure::NotYetValid,
                3 => TlsFailure::NameMismatch,
                4 => TlsFailure::Revoked,
                5 => TlsFailure::BadCertificate,
                6 => TlsFailure::AlertReceived(uint(p.remove(0)) as u8),
                7 => TlsFailure::HandshakeFailure,
                8 => TlsFailure::PeerMisbehaved,
                9 => TlsFailure::Truncated,
                _ => panic!("TLS tag"),
            })
        }
        17 => NetError::Fault(uint(payload.remove(0)) as u32),
        _ => panic!("network error tag"),
    })
}
fn addr(value: Value) -> SocketAddr {
    let Value::Record { mut fields, .. } = value else {
        panic!("SocketAddr record")
    };
    let ip = fields.remove(0).unwrap();
    let port = uint(fields.remove(0).unwrap()) as u16;
    let (tag, mut fields) = self::fields(ip);
    let octets = collection::elements(&fields.remove(0), BuiltinType::Inline)
        .unwrap()
        .iter()
        .cloned()
        .map(|v| uint(v) as u8)
        .collect::<Vec<_>>();
    SocketAddr {
        ip: match tag {
            0 => std::net::Ipv4Addr::from(<[u8; 4]>::try_from(octets).unwrap()).into(),
            1 => std::net::Ipv6Addr::from(<[u8; 16]>::try_from(octets).unwrap()).into(),
            _ => panic!("IP tag"),
        },
        port,
    }
}
pub fn decode(schema: &C, value: Value, updates: &[(usize, Value)]) -> C {
    match schema {
        C::Datagram(_) => C::Datagram(result(value).map(|v| {
            let Value::Record { mut fields, .. } = v else {
                panic!("UDP receive record")
            };
            let count = uint(fields[0].take().unwrap());
            let peer = addr(fields[1].take().unwrap());
            let Value::Bool(truncated) = fields[2].take().unwrap() else {
                panic!("UDP truncation")
            };
            let (_, buffer) = updates
                .iter()
                .find(|(n, _)| *n == 2)
                .expect("UDP output buffer");
            let bytes: Vec<_> = collection::elements(buffer, BuiltinType::List)
                .unwrap()
                .into_iter()
                .map(|v| uint(v) as u8)
                .collect();
            assert_eq!(bytes.len() as u64, count);
            (bytes, peer, truncated)
        })),
        C::NetHandle(_) => C::NetHandle(result(value).map(id)),
        C::Count(_) => C::Count(result(value).map(uint)),
        C::NetUnit(_) => C::NetUnit(result(value).map(|v| match v {
            Value::Unit => (),
            Value::Handle { kind, .. } if kind == "Stream" => (),
            _ => panic!("unit or upgraded Stream result"),
        })),
        C::Address(_) => C::Address(result(value).map(addr)),
        C::Addresses(_) => C::Addresses(result(value).map(|v| {
            collection::elements(&v, BuiltinType::List)
                .unwrap()
                .into_iter()
                .map(addr)
                .collect()
        })),
        C::NetBytes(_) => C::NetBytes(result(value).map(|v| {
            let count = uint(v);
            let (_, buffer) = updates
                .iter()
                .find(|(n, _)| *n == 2)
                .expect("successful read updates buffer");
            let bytes = collection::elements(buffer, BuiltinType::List)
                .unwrap()
                .iter()
                .cloned()
                .map(|v| uint(v) as u8)
                .collect::<Vec<_>>();
            assert_eq!(bytes.len() as u64, count);
            bytes
        })),
        _ => panic!("not a network completion"),
    }
}

pub fn fault(completion: &C) -> Option<hirsel_interp::library::simulation::Fault> {
    let error = match completion {
        C::Datagram(Err(e))
        | C::NetHandle(Err(e))
        | C::Addresses(Err(e))
        | C::NetBytes(Err(e))
        | C::Count(Err(e))
        | C::NetUnit(Err(e))
        | C::Address(Err(e)) => e,
        _ => return None,
    };
    match error {
        NetError::Fault(code) => Some(hirsel_interp::library::simulation::Fault::Fault {
            code: *code,
            partial: 0,
        }),
        NetError::Canceled(cause) => {
            Some(hirsel_interp::library::simulation::Fault::Canceled(*cause))
        }
        _ => None,
    }
}
