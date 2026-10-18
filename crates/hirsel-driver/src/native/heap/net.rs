//! Driver-only conversion between H semantic values and the native scripted
//! network model. All policy and peer transitions live in hirsel-rt.
use super::{Io, IoGrant, io_ok};
use hirsel_heapcheck::{OpOutcome, OpValue};
use hirsel_lir::layout_types::ScalarKind;
use hirsel_rt::{
    io::{Completion as C, Request as R},
    net::*,
};
use hirsel_run_config::{NetClient, NetConnection, NetListener, NetPeer, NetStep, PeerTls};

fn word(v: &OpValue) -> Option<u64> {
    match v {
        OpValue::Bits { bits, .. } => u64::try_from(*bits).ok(),
        _ => None,
    }
}
fn bytes(v: &OpValue) -> Option<Vec<u8>> {
    match v {
        OpValue::Bytes(b) => Some(b.clone()),
        OpValue::Elements {
            scalar: ScalarKind::U8,
            bits,
        } => bits.iter().map(|&b| u8::try_from(b).ok()).collect(),
        OpValue::Sequence(items) => items.iter().map(|b| u8::try_from(word(b)?).ok()).collect(),
        _ => None,
    }
}
fn text(v: &OpValue) -> Option<String> {
    String::from_utf8(bytes(v)?).ok()
}
fn fields(v: &OpValue) -> Option<&[OpValue]> {
    match v {
        OpValue::Aggregate(v) => Some(v),
        _ => None,
    }
}
fn sequence(v: &OpValue) -> Option<Vec<OpValue>> {
    match v {
        OpValue::Sequence(v) => Some(v.clone()),
        OpValue::Elements { scalar, bits } => Some(
            bits.iter()
                .map(|&bits| OpValue::Bits {
                    scalar: *scalar,
                    bits,
                })
                .collect(),
        ),
        _ => None,
    }
}
fn variant(v: &OpValue) -> Option<(u32, &OpValue)> {
    match v {
        OpValue::Variant { tag, value } => Some((*tag, value)),
        _ => None,
    }
}
fn ip(v: &OpValue) -> Option<std::net::IpAddr> {
    let (tag, payload) = variant(v)?;
    let b = bytes(payload)?;
    Some(match tag {
        0 => std::net::Ipv4Addr::from(<[u8; 4]>::try_from(b).ok()?).into(),
        1 => std::net::Ipv6Addr::from(<[u8; 16]>::try_from(b).ok()?).into(),
        _ => return None,
    })
}
fn addr(v: &OpValue) -> Option<SocketAddr> {
    let [address, port] = fields(v)? else {
        return None;
    };
    Some(SocketAddr {
        ip: ip(address)?,
        port: u16::try_from(word(port)?).ok()?,
    })
}
fn timeout(v: &OpValue) -> Option<NetTimeout> {
    let (tag, value) = variant(v)?;
    match tag {
        0 => Some(NetTimeout::Unbounded),
        1 => Some(NetTimeout::Nanos(word(value)?)),
        _ => None,
    }
}
fn tls_failure(v: &OpValue) -> Option<TlsFailure> {
    Some(match variant(v)? {
        (0, _) => TlsFailure::UnknownIssuer,
        (1, _) => TlsFailure::Expired,
        (2, _) => TlsFailure::NotYetValid,
        (3, _) => TlsFailure::NameMismatch,
        (4, _) => TlsFailure::Revoked,
        (5, _) => TlsFailure::BadCertificate,
        (6, alert) => TlsFailure::AlertReceived(u8::try_from(word(alert)?).ok()?),
        (7, _) => TlsFailure::HandshakeFailure,
        (8, _) => TlsFailure::PeerMisbehaved,
        (9, _) => TlsFailure::Truncated,
        _ => return None,
    })
}
fn steps(v: &OpValue) -> Option<Vec<NetStep>> {
    sequence(v)?
        .iter()
        .map(|s| {
            Some(match variant(s)? {
                (0, b) => NetStep::Send(bytes(b)?),
                (1, b) => NetStep::Expect(bytes(b)?),
                (2, _) => NetStep::Stall,
                (3, _) => NetStep::Close,
                (4, _) => NetStep::Reset,
                (5, _) => NetStep::Truncate,
                _ => return None,
            })
        })
        .collect()
}
fn listener(v: &OpValue) -> Option<NetListener> {
    let [address, port, clients] = fields(v)? else {
        return None;
    };
    Some(NetListener {
        address: text(address)?,
        port: u16::try_from(word(port)?).ok()?,
        clients: sequence(clients)?
            .iter()
            .map(|c| {
                let [from, tls, s] = fields(c)? else {
                    return None;
                };
                let from = addr(from)?;
                Some(NetClient {
                    address: from.ip.to_string(),
                    port: from.port,
                    tls: peer_tls(tls)?,
                    steps: steps(s)?,
                })
            })
            .collect::<Option<_>>()?,
    })
}
fn peer_tls(v: &OpValue) -> Option<PeerTls> {
    Some(match variant(v)? {
        (0, _) => PeerTls::Plain,
        (1, names) => {
            PeerTls::Certificate(sequence(names)?.iter().map(text).collect::<Option<_>>()?)
        }
        (2, failure) => PeerTls::Fails(tls_failure(failure)?.into()),
        (3, presents) => {
            let [names, chain] = fields(presents)? else {
                return None;
            };
            PeerTls::Presents(
                sequence(names)?.iter().map(text).collect::<Option<_>>()?,
                bytes(chain)?,
            )
        }
        _ => return None,
    })
}
fn peer(v: &OpValue) -> Option<NetPeer> {
    let [address, port, resolves, tls, connections] = fields(v)? else {
        return None;
    };
    let tls = peer_tls(tls)?;
    let connections = sequence(connections)?
        .iter()
        .map(|v| {
            Some(match variant(v)? {
                (0, _) => NetConnection::Refused,
                (1, _) => NetConnection::Unanswered,
                (2, s) => NetConnection::Open(steps(s)?),
                _ => return None,
            })
        })
        .collect::<Option<_>>()?;
    Some(NetPeer {
        address: text(address)?,
        port: u16::try_from(word(port)?).ok()?,
        resolves_to: sequence(resolves)?
            .iter()
            .map(|v| Some(ip(v)?.to_string()))
            .collect::<Option<_>>()?,
        tls,
        connections,
    })
}
fn rule(v: &OpValue) -> Option<NetRule> {
    let [address, port, operation] = fields(v)? else {
        return None;
    };
    Some(NetRule {
        address: text(address)?,
        port: u16::try_from(word(port)?).ok()?,
        operation: match variant(operation)?.0 {
            0 => NetOperation::Connect,
            1 => NetOperation::ConnectTls,
            2 => NetOperation::Listen,
            3 => NetOperation::UdpBind,
            4 => NetOperation::UdpPeer,
            5 => NetOperation::ListenTls,
            6 => NetOperation::UnixConnect,
            7 => NetOperation::UnixListen,
            8 => NetOperation::UnixListenRemoveStale,
            _ => return None,
        },
    })
}
fn bits(scalar: ScalarKind, bits: impl Into<u128>) -> OpValue {
    OpValue::Bits {
        scalar,
        bits: bits.into(),
    }
}
fn v(tag: u32, value: OpValue) -> OpValue {
    OpValue::Variant {
        tag,
        value: Box::new(value),
    }
}
fn error(e: NetError) -> OpValue {
    use NetError::*;
    let (tag, payload) = match e {
        NotGranted => (0, OpValue::Unit),
        Canceled(cause) => (1, v(cause, OpValue::Unit)),
        InvalidInput => (2, OpValue::Unit),
        UnknownHost => (3, OpValue::Unit),
        ResolverFailure => (4, OpValue::Unit),
        ConnectionRefused => (5, OpValue::Unit),
        ConnectionReset => (6, OpValue::Unit),
        ConnectionAborted => (7, OpValue::Unit),
        NotConnected => (8, OpValue::Unit),
        AddrInUse => (9, OpValue::Unit),
        AddrNotAvailable => (10, OpValue::Unit),
        HostUnreachable => (11, OpValue::Unit),
        NetworkUnreachable => (12, OpValue::Unit),
        NetworkDown => (13, OpValue::Unit),
        BrokenPipe => (14, OpValue::Unit),
        TimedOut => (15, OpValue::Unit),
        Tls(e) => (
            16,
            match e {
                TlsFailure::AlertReceived(c) => v(6, bits(ScalarKind::U8, c)),
                e => v(
                    match e {
                        TlsFailure::UnknownIssuer => 0,
                        TlsFailure::Expired => 1,
                        TlsFailure::NotYetValid => 2,
                        TlsFailure::NameMismatch => 3,
                        TlsFailure::Revoked => 4,
                        TlsFailure::BadCertificate => 5,
                        TlsFailure::HandshakeFailure => 7,
                        TlsFailure::PeerMisbehaved => 8,
                        TlsFailure::Truncated => 9,
                        TlsFailure::AlertReceived(_) => unreachable!(),
                    },
                    OpValue::Unit,
                ),
            },
        ),
        Fault(c) => (17, bits(ScalarKind::U32, c)),
    };
    v(1, v(tag, payload))
}
fn result<T>(r: Result<T, NetError>, value: impl FnOnce(T) -> OpValue) -> OpValue {
    match r {
        Ok(x) => io_ok(value(x)),
        Err(e) => error(e),
    }
}
fn address(a: SocketAddr) -> OpValue {
    let (tag, b) = match a.ip {
        std::net::IpAddr::V4(ip) => (0, ip.octets().to_vec()),
        std::net::IpAddr::V6(ip) => (1, ip.octets().to_vec()),
    };
    OpValue::Aggregate(vec![
        v(
            tag,
            OpValue::Sequence(b.into_iter().map(|b| bits(ScalarKind::U8, b)).collect()),
        ),
        bits(ScalarKind::U16, a.port),
    ])
}
impl Io {
    pub(super) fn network_id(&self, value: &OpValue) -> Option<u64> {
        let OpValue::Handle(h) = value else {
            return None;
        };
        self.network_handles.get(h).copied()
    }
    pub(super) fn network_handle(&mut self, id: u64, owned: bool) -> OpValue {
        let external = self.next;
        self.next += 1;
        self.network_handles.insert(external, id);
        if owned {
            OpValue::OpenHandle(external)
        } else {
            OpValue::Handle(external)
        }
    }
    pub(super) fn net(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[OpValue],
        cancellation: Option<u32>,
    ) -> Option<OpOutcome> {
        let request = match (module, name, args) {
            (Some("test"), "net", [OpValue::Handle(test), peers, listeners])
                if matches!(self.grants.get(test), Some(IoGrant::Test)) =>
            {
                R::TestNet {
                    test: 1,
                    peers: sequence(peers)?.iter().map(peer).collect::<Option<_>>()?,
                    listeners: sequence(listeners)?
                        .iter()
                        .map(listener)
                        .collect::<Option<_>>()?,
                }
            }
            (Some("io"), "net", [OpValue::Handle(1)]) => R::Net { io: 1 },
            (Some("net"), "narrow", [n, rules]) => R::NetNarrow {
                net: self.network_id(n)?,
                rules: sequence(rules)?.iter().map(rule).collect::<Option<_>>()?,
            },
            (Some("net"), "resolve", [n, host, port]) => R::Resolve {
                net: self.network_id(n)?,
                host: text(host)?,
                port: u16::try_from(word(port)?).ok()?,
            },
            (Some("net"), "connect", [n, host, port, t]) => R::Connect {
                net: self.network_id(n)?,
                host: text(host)?,
                port: u16::try_from(word(port)?).ok()?,
                timeout: timeout(t)?,
            },
            (Some("net"), "connect_addr", [n, a, t]) => R::ConnectAddr {
                net: self.network_id(n)?,
                addr: addr(a)?,
                timeout: timeout(t)?,
            },
            (Some("net"), "read", [n, s, _, max]) => R::NetRead {
                net: self.network_id(n)?,
                stream: self.network_id(s)?,
                max: word(max)?,
            },
            (Some("$intrinsic"), "NetReadInto", [n, s, _, capacity]) => R::NetRead {
                net: self.network_id(n)?,
                stream: self.network_id(s)?,
                max: word(capacity)?,
            },
            (Some("net"), "read_into", [n, s, buf]) => R::NetRead {
                net: self.network_id(n)?,
                stream: self.network_id(s)?,
                max: bytes(buf)?.len() as u64,
            },
            (Some("net"), "write", [n, s, b]) => R::NetWrite {
                net: self.network_id(n)?,
                stream: self.network_id(s)?,
                bytes: bytes(b)?,
            },
            (Some("net"), "write_all", [n, s, b]) => R::NetWriteAll {
                net: self.network_id(n)?,
                stream: self.network_id(s)?,
                bytes: bytes(b)?,
            },
            (Some("net"), "shutdown", [n, s, how]) => R::Shutdown {
                net: self.network_id(n)?,
                stream: self.network_id(s)?,
                how: match variant(how)?.0 {
                    0 => ShutdownHow::Read,
                    1 => ShutdownHow::Write,
                    2 => ShutdownHow::Both,
                    _ => return None,
                },
            },
            (Some("net"), "set_read_timeout" | "set_write_timeout", [n, s, t]) => R::SetTimeout {
                net: self.network_id(n)?,
                stream: self.network_id(s)?,
                which: if name == "set_read_timeout" {
                    TimeoutKind::Read
                } else {
                    TimeoutKind::Write
                },
                timeout: timeout(t)?,
            },
            (Some("net"), "peer_addr", [n, s]) => R::PeerAddr {
                net: self.network_id(n)?,
                stream: self.network_id(s)?,
            },
            (Some("net"), "local_addr", [n, l]) => R::LocalAddr {
                net: self.network_id(n)?,
                listener: self.network_id(l)?,
            },
            (Some("net"), "udp_bind", [n, a]) => R::UdpBind {
                net: self.network_id(n)?,
                addr: addr(a)?,
            },
            (Some("udp"), "send_to", [n, s, b, a]) => R::UdpSend {
                net: self.network_id(n)?,
                socket: self.network_id(s)?,
                bytes: bytes(b)?,
                addr: addr(a)?,
            },
            (Some("udp"), "recv_from", [n, s, _, max]) => R::UdpRecv {
                net: self.network_id(n)?,
                socket: self.network_id(s)?,
                max: word(max)?,
            },
            (Some("udp"), "close", [_, s]) => R::Close {
                handle: self.network_id(s)?,
            },
            (Some("net"), "listen", [n, a]) => R::Listen {
                net: self.network_id(n)?,
                addr: addr(a)?,
            },
            (Some("net"), "unix_connect", [n, OpValue::Handle(path)]) => R::UnixConnect {
                net: self.network_id(n)?,
                path: self.paths.get(path.checked_sub(1)? as usize)?.clone(),
            },
            (Some("net"), "unix_listen", [n, OpValue::Handle(path)]) => R::UnixListen {
                net: self.network_id(n)?,
                path: self.paths.get(path.checked_sub(1)? as usize)?.clone(),
            },
            (Some("net"), "accept", [n, l]) => R::Accept {
                net: self.network_id(n)?,
                listener: self.network_id(l)?,
            },
            (Some("net"), "close", [_, s]) => R::Close {
                handle: self.network_id(s)?,
            },
            (Some("tls"), "close", [n, s]) => R::TlsClose {
                net: self.network_id(n)?,
                stream: self.network_id(s)?,
            },
            (Some("tls"), "connect" | "connect_as", [n, s, host, roots, identity @ ..]) => {
                let stream = self.network_id(s)?;
                let roots = match variant(roots)? {
                    (0, _) => RootsDigest::WebPki("webpki-roots@1.0.9".into()),
                    (1, b) => {
                        use sha2::Digest;
                        let pem = bytes(b)?;
                        hirsel_rt::io::Adapter::tls_roots(&mut self.network, stream, &pem);
                        RootsDigest::Pem(sha2::Sha256::digest(&pem).into())
                    }
                    _ => return None,
                };
                let identity = match (name, identity) {
                    ("connect", []) => None,
                    ("connect_as", [identity]) => {
                        let [chain, key] = fields(identity)? else {
                            return None;
                        };
                        let (chain, key) = (bytes(chain)?, bytes(key)?);
                        hirsel_rt::io::Adapter::tls_identity(
                            &mut self.network,
                            stream,
                            &chain,
                            &key,
                        );
                        Some(hirsel_rt::net_tls::chain_digest(&chain))
                    }
                    _ => return None,
                };
                R::TlsConnect {
                    net: self.network_id(n)?,
                    stream,
                    server_name: text(host)?,
                    roots,
                    identity,
                }
            }
            (Some("tls"), "accept", [n, s, server]) => {
                let [identities, clients] = fields(server)? else {
                    return None;
                };
                let identities = sequence(identities)?
                    .iter()
                    .map(|identity| {
                        let [chain, key] = fields(identity)? else {
                            return None;
                        };
                        Some((bytes(chain)?, bytes(key)?))
                    })
                    .collect::<Option<Vec<_>>>()?;
                let identities: Vec<(&[u8], &[u8])> =
                    identities.iter().map(|(c, k)| (&c[..], &k[..])).collect();
                let (tag, trust) = variant(clients)?;
                let trust = if tag == 0 { vec![] } else { bytes(trust)? };
                let clients = match tag {
                    0 => ClientAuth::Anonymous,
                    1 => ClientAuth::Request(&trust[..]),
                    2 => ClientAuth::Require(&trust[..]),
                    _ => return None,
                };
                let stream = self.network_id(s)?;
                hirsel_rt::io::Adapter::tls_server(&mut self.network, stream, &identities, clients);
                R::TlsAccept {
                    net: self.network_id(n)?,
                    stream,
                    server: hirsel_rt::net::TlsServerDigest::of(&identities, clients),
                }
            }
            (Some("tls"), "peer", [n, s]) => R::TlsPeer {
                net: self.network_id(n)?,
                stream: self.network_id(s)?,
            },
            _ => return None,
        };
        let completion = match self.script(request, cancellation) {
            Ok(c) => c,
            Err(outcome) => return Some(outcome),
        };
        let value = match completion {
            C::Handle(r) if name == "net" => {
                if module == Some("test") {
                    self.network_handle(r.ok()?, false)
                } else {
                    io_ok(self.network_handle(r.ok()?, false))
                }
            }
            C::Listened(r) => result(r, |(id, _)| self.network_handle(id, true)),
            C::NetHandle(r) => result(r, |id| self.network_handle(id, name != "narrow")),
            C::Datagram(r) => match r {
                Err(e) => error(e),
                Ok((b, peer, truncated)) => {
                    let count = b.len() as u64;
                    let mut buffer = bytes(&args[2])?;
                    buffer.extend_from_slice(&b);
                    return Some(OpOutcome::Updated {
                        value: io_ok(OpValue::Aggregate(vec![
                            bits(ScalarKind::U64, count),
                            address(peer),
                            bits(ScalarKind::Bool, u8::from(truncated)),
                        ])),
                        arguments: vec![(
                            2,
                            OpValue::Elements {
                                scalar: ScalarKind::U8,
                                bits: buffer.into_iter().map(u128::from).collect(),
                            },
                        )],
                    });
                }
            },
            C::NetBytes(r) => match r {
                Ok(b) => {
                    let count = b.len() as u64;
                    let mut buf = bytes(&args[2])?;
                    if name == "read_into" || name == "NetReadInto" {
                        buf.get_mut(..b.len())?.copy_from_slice(&b);
                    } else {
                        buf.extend_from_slice(&b);
                    }
                    return Some(OpOutcome::Updated {
                        value: io_ok(bits(ScalarKind::U64, count)),
                        arguments: vec![(
                            2,
                            OpValue::Elements {
                                scalar: ScalarKind::U8,
                                bits: buf.into_iter().map(u128::from).collect(),
                            },
                        )],
                    });
                }
                Err(e) => error(e),
            },
            C::NetUnit(r) if module == Some("tls") && name != "close" => {
                result(r, |_| OpValue::TakenHandle { argument: 1 })
            }
            C::NetUnit(r) => result(r, |()| OpValue::Unit),
            C::Certificates(r) => result(r, |chain| match chain {
                Some(chain) => v(1, OpValue::ByteSequence(chain)),
                None => v(0, OpValue::Unit),
            }),
            C::Count(r) => result(r, |n| bits(ScalarKind::U64, n)),
            C::Address(r) => result(r, address),
            C::Addresses(r) => result(r, |a| {
                OpValue::Sequence(a.into_iter().map(address).collect())
            }),
            C::Accepted(r) => result(r, |(id, a)| {
                OpValue::Aggregate(vec![self.network_handle(id, true), address(a)])
            }),
            C::Unit(Ok(())) => OpValue::Unit,
            // A scripted client's unmet Expect (docs/spec/core-library.md).
            C::Checked(false) if name == "close" || module == Some("tls") => {
                if let OpValue::Handle(h) = args[1] {
                    self.network_handles.remove(&h);
                }
                return Some(OpOutcome::Complete {
                    result: Err("LANG-9".into()),
                    consumed: vec![1],
                });
            }
            C::Checked(false) => return Some(OpOutcome::Fail("LANG-9".into())),
            _ => return Some(OpOutcome::Unsupported),
        };
        let consumed = module == Some("tls")
            && name != "peer"
            && (name == "close" || matches!(value, OpValue::Variant { tag: 1, .. }))
            || matches!(module, Some("net" | "udp")) && name == "close";
        Some(if consumed {
            if let OpValue::Handle(h) = args[1] {
                self.network_handles.remove(&h);
            }
            OpOutcome::Complete {
                result: Ok(value),
                consumed: vec![1],
            }
        } else {
            OpOutcome::Value(value)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model(tls: OpValue, connections: Vec<OpValue>) -> (Io, OpValue) {
        let mut io = Io::for_launch(
            &crate::native::link::LaunchKind::Test,
            7,
            0,
            &super::super::HeapWorld::default(),
        )
        .unwrap();
        let peers = OpValue::Sequence(vec![OpValue::Aggregate(vec![
            OpValue::Bytes(b"example.com".to_vec()),
            bits(ScalarKind::U16, 80u16),
            OpValue::Sequence(vec![v(
                0,
                OpValue::Sequence(
                    [127u8, 0, 0, 1]
                        .into_iter()
                        .map(|b| bits(ScalarKind::U8, b))
                        .collect(),
                ),
            )]),
            tls,
            OpValue::Sequence(connections),
        ])]);
        let OpOutcome::Value(n) = io
            .net(
                Some("test"),
                "net",
                &[OpValue::Handle(1), peers, OpValue::Sequence(vec![])],
                None,
            )
            .unwrap()
        else {
            panic!()
        };
        (io, n)
    }
    fn open(io: &mut Io, n: &OpValue) -> OpValue {
        let OpOutcome::Value(OpValue::Variant { tag: 0, value }) = io
            .net(
                Some("net"),
                "connect",
                &[
                    n.clone(),
                    OpValue::Bytes(b"example.com".to_vec()),
                    bits(ScalarKind::U16, 80u16),
                    v(0, OpValue::Unit),
                ],
                None,
            )
            .unwrap()
        else {
            panic!()
        };
        let OpValue::OpenHandle(h) = *value else {
            panic!()
        };
        OpValue::Handle(h)
    }
    fn value(io: &mut Io, name: &str, args: &[OpValue]) -> OpValue {
        let OpOutcome::Value(v) = io.net(Some("net"), name, args, None).unwrap() else {
            panic!()
        };
        v
    }
    #[test]
    fn scripted_reads_keep_chunks_and_forks_keep_independent_peer_progress() {
        let (mut io, n) = model(
            v(0, OpValue::Unit),
            vec![v(
                2,
                OpValue::Sequence(vec![
                    v(1, OpValue::Bytes(b"GET".to_vec())),
                    v(0, OpValue::Bytes(b"abc".to_vec())),
                    v(0, OpValue::Bytes(b"de".to_vec())),
                    v(3, OpValue::Unit),
                ]),
            )],
        );
        let s = open(&mut io, &n);
        assert_eq!(
            value(
                &mut io,
                "write_all",
                &[n.clone(), s.clone(), OpValue::Bytes(b"GET".to_vec())]
            ),
            io_ok(OpValue::Unit)
        );
        let mut fork = io.clone();
        for (model, expected) in [(&mut io, b"abc".as_slice()), (&mut fork, b"abc")] {
            let OpOutcome::Updated { value, arguments } = model
                .net(
                    Some("net"),
                    "read",
                    &[
                        n.clone(),
                        s.clone(),
                        OpValue::Elements {
                            scalar: ScalarKind::U8,
                            bits: vec![99],
                        },
                        bits(ScalarKind::U64, 99u64),
                    ],
                    None,
                )
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(value, io_ok(bits(ScalarKind::U64, 3u64)));
            assert_eq!(
                arguments,
                vec![(
                    2,
                    OpValue::Elements {
                        scalar: ScalarKind::U8,
                        bits: [b"c".as_slice(), expected]
                            .concat()
                            .into_iter()
                            .map(u128::from)
                            .collect()
                    }
                )]
            );
        }
        assert_eq!(
            value(&mut io, "peer_addr", &[n, s]),
            io_ok(address(SocketAddr {
                ip: std::net::Ipv4Addr::LOCALHOST.into(),
                port: 80
            }))
        );
    }
    #[test]
    fn grants_timeouts_and_mismatch_use_the_shared_runtime_policy() {
        let (mut io, n) = model(
            v(0, OpValue::Unit),
            vec![
                v(1, OpValue::Unit),
                v(0, OpValue::Unit),
                v(
                    2,
                    OpValue::Sequence(vec![v(1, OpValue::Bytes(b"yes".to_vec()))]),
                ),
            ],
        );
        let connect = |io: &mut Io, t| {
            value(
                io,
                "connect",
                &[
                    n.clone(),
                    OpValue::Bytes(b"example.com".to_vec()),
                    bits(ScalarKind::U16, 80u16),
                    t,
                ],
            )
        };
        assert_eq!(
            connect(&mut io, v(1, bits(ScalarKind::U64, 0u64))),
            error(NetError::InvalidInput)
        );
        assert_eq!(
            connect(&mut io, v(1, bits(ScalarKind::U64, 1u64))),
            error(NetError::TimedOut)
        );
        assert_eq!(
            connect(&mut io, v(0, OpValue::Unit)),
            error(NetError::ConnectionRefused)
        );
        assert_eq!(
            value(
                &mut io,
                "connect",
                &[
                    n.clone(),
                    OpValue::Bytes(b"elsewhere.com".to_vec()),
                    bits(ScalarKind::U16, 80u16),
                    v(0, OpValue::Unit)
                ]
            ),
            error(NetError::NotGranted)
        );
        let s = open(&mut io, &n);
        assert_eq!(
            value(
                &mut io,
                "write",
                &[n.clone(), s.clone(), OpValue::Bytes(b"no!".to_vec())]
            ),
            io_ok(bits(ScalarKind::U64, 3u64))
        );
        assert_eq!(
            value(&mut io, "peer_addr", &[n, s]),
            error(NetError::ConnectionReset)
        );
    }
    #[test]
    fn tls_transfers_success_and_consumes_failure_and_close() {
        for (names, success) in [
            (vec![OpValue::Bytes(b"example.com".to_vec())], true),
            (vec![OpValue::Bytes(b"other.com".to_vec())], false),
        ] {
            let (mut io, n) = model(
                v(1, OpValue::Sequence(names)),
                vec![v(2, OpValue::Sequence(vec![]))],
            );
            let s = open(&mut io, &n);
            assert_eq!(
                value(
                    &mut io,
                    "write",
                    &[n.clone(), s.clone(), OpValue::Bytes(vec![])]
                ),
                error(NetError::NotGranted)
            );
            let outcome = io
                .net(
                    Some("tls"),
                    "connect",
                    &[
                        n.clone(),
                        s.clone(),
                        OpValue::Bytes(b"example.com".to_vec()),
                        v(0, OpValue::Unit),
                    ],
                    None,
                )
                .unwrap();
            if success {
                assert!(matches!(
                    outcome,
                    OpOutcome::Value(OpValue::Variant { tag: 0, .. })
                ));
                assert!(
                    matches!(io.net(Some("tls"),"close",&[n,s], None).unwrap(),OpOutcome::Complete {consumed,..} if consumed==[1])
                );
            } else {
                assert!(matches!(outcome,OpOutcome::Complete {consumed,..} if consumed==[1]));
            }
        }
    }
}
