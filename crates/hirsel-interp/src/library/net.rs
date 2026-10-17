//! Frozen scripted peers, an original state machine:
//! neither Rust std nor Zig has this peer language. Socket conventions follow
//! Rust library/std/src/net/tcp.rs; its timeout/read/half-close vectors are in
//! net_tests.rs. No OS networking, crypto, wall time or host identity is used.
use super::{
    collection,
    dispatch::{
        arg, bytes, encode_arguments, encode_value, error, index, ok, text, u64_value, variant,
    },
    simulation::{Fault, Grant, Simulation},
};
use crate::{Error, Integer, OperationCall, OperationContext, OperationResult, Value, invalid};
use hirsel_run_config::{
    NetConnection, NetGrant, NetPeer, NetRule, NetStep, PeerTls, ProviderContract, TlsFailure,
    canonical_net_address as canonical_address,
};
use hirsel_types::{BuiltinType, Primitive};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    net::IpAddr,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct World {
    peers: Vec<NetPeer>,
    next: Vec<usize>,
}
impl World {
    pub fn new(peers: Vec<NetPeer>) -> Self {
        let next = vec![0; peers.len()];
        Self { peers, next }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ScriptStream {
    pub net: u64,
    host: String,
    port: u16,
    peer: PeerTls,
    requires_tls: bool,
    tls: bool,
    steps: VecDeque<NetStep>,
    offset: usize,
    written: VecDeque<u8>,
    read_timeout: Option<u64>,
    write_timeout: Option<u64>,
    read_closed: bool,
    write_closed: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum NetFault {
    NotGranted,
    InvalidInput,
    UnknownHost,
    ConnectionRefused,
    ConnectionReset,
    BrokenPipe,
    TimedOut,
    Canceled(u32),
    Fault(u32),
    Tls(TlsFailure),
    Blocked,
}
type NetResult<T> = Result<T, NetFault>;
impl ScriptStream {
    fn new(
        net: u64,
        host: String,
        port: u16,
        peer: PeerTls,
        requires_tls: bool,
        steps: Vec<NetStep>,
    ) -> Self {
        Self {
            net,
            host,
            port,
            peer,
            requires_tls,
            tls: false,
            steps: steps.into(),
            offset: 0,
            written: VecDeque::new(),
            read_timeout: None,
            write_timeout: None,
            read_closed: false,
            write_closed: false,
        }
    }
    fn advance(&mut self) {
        loop {
            match self.steps.front() {
                Some(NetStep::Send(b)) if b.len() == self.offset => {
                    self.steps.pop_front();
                    self.offset = 0;
                }
                Some(NetStep::Expect(b)) if self.written.len() >= b.len() => {
                    let matches = self.written.iter().take(b.len()).eq(b.iter());
                    if !matches {
                        self.steps.clear();
                        self.steps.push_back(NetStep::Reset);
                        break;
                    }
                    self.written.drain(..b.len());
                    self.steps.pop_front();
                }
                _ => break,
            }
        }
    }
    fn check(&mut self) -> NetResult<()> {
        if self.requires_tls && !self.tls {
            return Err(NetFault::NotGranted);
        }
        self.advance();
        if matches!(self.steps.front(), Some(NetStep::Reset)) {
            Err(NetFault::ConnectionReset)
        } else {
            Ok(())
        }
    }
    fn read(&mut self, max: usize) -> NetResult<Vec<u8>> {
        if max == 0 {
            return Err(NetFault::InvalidInput);
        }
        self.check()?;
        if self.read_closed {
            return Ok(vec![]);
        }
        match self.steps.front() {
            Some(NetStep::Send(bytes)) => {
                let end = self.offset + max.min(bytes.len() - self.offset);
                let out = bytes[self.offset..end].to_vec();
                self.offset = end;
                Ok(out)
            }
            None | Some(NetStep::Close) => Ok(vec![]),
            // Only a secured stream has a close_notify to miss.
            Some(NetStep::Truncate) if self.tls => Err(NetFault::Tls(TlsFailure::Truncated)),
            Some(NetStep::Truncate) => Ok(vec![]),
            _ => Err(if self.read_timeout.is_some() {
                NetFault::TimedOut
            } else {
                NetFault::Blocked
            }),
        }
    }
    fn write(&mut self, bytes: &[u8]) -> NetResult<u64> {
        self.check()?;
        if self.write_closed {
            return Err(NetFault::BrokenPipe);
        }
        self.written.extend(bytes);
        self.advance();
        Ok(bytes.len() as u64)
    }
}
fn timeout(value: &Value) -> Result<NetResult<Option<u64>>, Error> {
    let (tag, fields) = variant_fields(value)?;
    match (tag, fields) {
        (0, []) => Ok(Ok(None)),
        (1, [Some(v)]) => {
            let n = index(v)? as u64;
            Ok(if n == 0 {
                Err(NetFault::InvalidInput)
            } else {
                Ok(Some(n))
            })
        }
        _ => Err(invalid("NetTimeout representation")),
    }
}
fn variant_fields(v: &Value) -> Result<(u32, &[Option<Value>]), Error> {
    match v {
        Value::Variant { variant, fields } => Ok((*variant, fields)),
        _ => Err(invalid("network variant representation")),
    }
}
fn fields<'a>(v: &'a Value, name: &str, len: usize) -> Result<Vec<&'a Value>, Error> {
    if let Value::Record { definition, fields } = v
        && *definition == hirsel_syntax::DefPath::core(name)
        && fields.len() == len
    {
        return fields
            .iter()
            .map(|f| f.as_ref().ok_or_else(|| invalid("dead network field")))
            .collect();
    }
    Err(invalid(format!("{name} representation")))
}
fn port(v: &Value) -> Result<u16, Error> {
    match v {
        Value::Integer(n) if n.kind() == Primitive::U16 => Ok(n.bits() as u16),
        _ => Err(invalid("network port requires u16")),
    }
}
fn ip(v: &Value) -> Result<IpAddr, Error> {
    let (tag, f) = variant_fields(v)?;
    let [Some(v)] = f else {
        return Err(invalid("IpAddr payload"));
    };
    let values = collection::elements(v, BuiltinType::Inline)?;
    let bytes: Vec<u8> = values
        .iter()
        .map(|v| match v {
            Value::Integer(i) if i.kind() == Primitive::U8 => Ok(i.bits() as u8),
            _ => Err(invalid("IP octet")),
        })
        .collect::<Result<_, _>>()?;
    match tag {
        0 => Ok(IpAddr::V4(std::net::Ipv4Addr::from(
            <[u8; 4]>::try_from(bytes).map_err(|_| invalid("IPv4 width"))?,
        ))),
        1 => Ok(IpAddr::V6(std::net::Ipv6Addr::from(
            <[u8; 16]>::try_from(bytes).map_err(|_| invalid("IPv6 width"))?,
        ))),
        _ => Err(invalid("IpAddr ordinal")),
    }
}
fn ip_value(ip: IpAddr) -> Value {
    let (tag, octets) = match ip {
        IpAddr::V4(ip) => (0, ip.octets().to_vec()),
        IpAddr::V6(ip) => (1, ip.octets().to_vec()),
    };
    variant(
        tag,
        vec![collection::sequence(
            BuiltinType::Inline,
            octets
                .into_iter()
                .map(|b| Value::Integer(Integer::new(Primitive::U8, b.into()).unwrap())),
        )],
    )
}
fn addr_value(ip: IpAddr, port: u16) -> Value {
    Value::Record {
        definition: hirsel_syntax::DefPath::core("SocketAddr"),
        fields: vec![
            Some(ip_value(ip)),
            Some(Value::Integer(
                Integer::new(Primitive::U16, port.into()).unwrap(),
            )),
        ],
    }
}
fn peer(v: &Value) -> Result<NetPeer, Error> {
    let f = fields(v, "NetPeer", 5)?;
    let (tag, tls) = variant_fields(f[3])?;
    let tls = match (tag, tls) {
        (0, []) => PeerTls::Plain,
        (1, [Some(names)]) => PeerTls::Certificate(
            collection::elements(names, BuiltinType::List)?
                .iter()
                .map(|v| text(v).map(str::to_owned))
                .collect::<Result<_, _>>()?,
        ),
        (2, [Some(failure)]) => PeerTls::Fails(tls_failure(failure)?),
        _ => return Err(invalid("PeerTls representation")),
    };
    let mut connections = vec![];
    for c in collection::elements(f[4], BuiltinType::List)? {
        let (tag, params) = variant_fields(&c)?;
        connections.push(match (tag, params) {
            (0, []) => NetConnection::Refused,
            (1, []) => NetConnection::Unanswered,
            (2, [Some(steps)]) => NetConnection::Open(
                collection::elements(steps, BuiltinType::List)?
                    .iter()
                    .map(|v| {
                        let (tag, p) = variant_fields(v)?;
                        Ok(match (tag, p) {
                            (0, [Some(b)]) => NetStep::Send(bytes(b)?.to_vec()),
                            (1, [Some(b)]) => NetStep::Expect(bytes(b)?.to_vec()),
                            (2, []) => NetStep::Stall,
                            (3, []) => NetStep::Close,
                            (4, []) => NetStep::Reset,
                            (5, []) => NetStep::Truncate,
                            _ => return Err(invalid("NetStep representation")),
                        })
                    })
                    .collect::<Result<_, Error>>()?,
            ),
            _ => return Err(invalid("NetConnection representation")),
        });
    }
    let address = text(f[0])?.to_owned();
    if !canonical_address(&address) {
        return Err(invalid("test.net noncanonical peer address"));
    }
    Ok(NetPeer {
        address,
        port: port(f[1])?,
        resolves_to: collection::elements(f[2], BuiltinType::List)?
            .iter()
            .map(|v| ip(v).map(|ip| ip.to_string()))
            .collect::<Result<_, _>>()?,
        tls,
        connections,
    })
}
const TLS_FAILURES: [TlsFailure; 10] = [
    TlsFailure::UnknownIssuer,
    TlsFailure::Expired,
    TlsFailure::NotYetValid,
    TlsFailure::NameMismatch,
    TlsFailure::Revoked,
    TlsFailure::BadCertificate,
    TlsFailure::AlertReceived(0),
    TlsFailure::HandshakeFailure,
    TlsFailure::PeerMisbehaved,
    TlsFailure::Truncated,
];
fn tls_failure(v: &Value) -> Result<TlsFailure, Error> {
    match variant_fields(v)? {
        (6, [Some(Value::Integer(n))]) if n.kind() == Primitive::U8 => {
            Ok(TlsFailure::AlertReceived(n.bits() as u8))
        }
        (6, _) => Err(invalid("TlsFailure representation")),
        (tag, []) => TLS_FAILURES
            .get(tag as usize)
            .copied()
            .ok_or_else(|| invalid("TlsFailure representation")),
        _ => Err(invalid("TlsFailure representation")),
    }
}
fn tls_failure_value(failure: TlsFailure) -> Value {
    match failure {
        TlsFailure::AlertReceived(alert) => variant(
            6,
            vec![Value::Integer(
                Integer::new(Primitive::U8, alert.into()).unwrap(),
            )],
        ),
        _ => variant(
            TLS_FAILURES.iter().position(|f| *f == failure).unwrap() as u32,
            vec![],
        ),
    }
}
fn rules(v: &Value) -> Result<Vec<NetRule>, Error> {
    collection::elements(v, BuiltinType::List)?
        .iter()
        .map(|v| {
            let f = fields(v, "NetRule", 3)?;
            let (op, payload) = variant_fields(f[2])?;
            if !payload.is_empty() || op > 2 {
                return Err(invalid("NetOperation representation"));
            }
            Ok(NetRule {
                address: text(f[0])?.into(),
                service: port(f[1])?.to_string(),
                operation: ["connect", "connect_tls", "listen"][op as usize].into(),
            })
        })
        .collect()
}
fn injected(fault: Fault) -> Result<NetFault, Error> {
    Ok(match fault {
        Fault::Canceled(cause) => NetFault::Canceled(cause),
        Fault::Fault { code, .. } => NetFault::Fault(code),
        Fault::File { .. } => return Err(invalid("file error supplied to network")),
    })
}
fn result(value: NetResult<Value>) -> Result<Value, NetFault> {
    value.map(ok).or_else(|e| match e {
        NetFault::Blocked => Err(e),
        NetFault::Canceled(cause) => Ok(error(variant(1, vec![variant(cause, vec![])]))),
        NetFault::Fault(code) => Ok(error(variant(17, vec![super::u32_value(code)]))),
        NetFault::NotGranted => Ok(error(variant(0, vec![]))),
        NetFault::InvalidInput => Ok(error(variant(2, vec![]))),
        NetFault::UnknownHost => Ok(error(variant(3, vec![]))),
        NetFault::ConnectionRefused => Ok(error(variant(5, vec![]))),
        NetFault::ConnectionReset => Ok(error(variant(6, vec![]))),
        NetFault::BrokenPipe => Ok(error(variant(14, vec![]))),
        NetFault::TimedOut => Ok(error(variant(15, vec![]))),
        NetFault::Tls(failure) => Ok(error(variant(16, vec![tls_failure_value(failure)]))),
    })
}
impl Simulation {
    pub(super) fn net_root(&mut self) -> Value {
        let rules = self
            .config
            .net
            .as_ref()
            .map(|n| n.rules.clone())
            .unwrap_or_default();
        self.handle("Net", Grant::Net { world: 0, rules })
    }
    fn net_grant(&self, v: &Value) -> Result<(u64, u64, Vec<NetRule>), Error> {
        let id = self.id(v, "Net")?;
        let Some(Grant::Net { world, rules }) = self.grants.get(&id) else {
            return Err(invalid("Net grant"));
        };
        Ok((id, *world, rules.clone()))
    }
    fn stream_id(&self, n: u64, v: &Value) -> Result<u64, Error> {
        let id = self.id(v, "Stream")?;
        if self.net_streams.get(&id).is_none_or(|s| s.net != n) {
            return Err(invalid("wrong stream Net"));
        }
        Ok(id)
    }
    pub(super) fn net_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        cx: &mut OperationContext<'_>,
        cancellation: Option<u32>,
    ) -> Result<OperationResult, Error> {
        let a = call.arguments;
        if name == "test.net" {
            self.id(arg(a, 0)?, "Test")?;
            let peers = collection::elements(arg(a, 1)?, BuiltinType::List)?
                .iter()
                .map(peer)
                .collect::<Result<Vec<_>, _>>()?;
            validate_peers(&peers)?;
            let rules = peers
                .iter()
                .map(|p| NetRule {
                    address: p.address.clone(),
                    service: p.port.to_string(),
                    operation: if matches!(p.tls, PeerTls::Plain) {
                        "connect"
                    } else {
                        "connect_tls"
                    }
                    .into(),
                })
                .collect();
            let world = self.next_id;
            self.net_worlds.insert(world, World::new(peers));
            let value = self.handle("Net", Grant::Net { world, rules });
            self.record("Test", name, encode_arguments(a), encode_value(&value), cx)?;
            return Ok(OperationResult::returned(value));
        }
        let (net, world, ruleset) = self.net_grant(arg(a, 0)?)?;
        let mut updates = vec![];
        let mut recorded = None;
        let outcome = 'operation: {
            match name {
                "net.narrow" => {
                    let rules = rules(arg(a, 1)?)?;
                    if rules.iter().any(|r| !canonical_address(&r.address)) {
                        Err(NetFault::InvalidInput)
                    } else if !rules.iter().all(|r| {
                        ruleset.iter().any(|p| {
                            p.address == r.address
                                && p.service == r.service
                                && (p.operation == r.operation
                                    || p.operation == "connect" && r.operation == "connect_tls")
                        })
                    }) {
                        Err(NetFault::NotGranted)
                    } else {
                        Ok(self.handle("Net", Grant::Net { world, rules }))
                    }
                }
                "net.resolve" | "net.connect" | "net.connect_addr" => {
                    let (host, port, timeout) = if name == "net.connect_addr" {
                        let f = fields(arg(a, 1)?, "SocketAddr", 2)?;
                        (ip(f[0])?.to_string(), port(f[1])?, timeout(arg(a, 2)?)?)
                    } else {
                        (
                            text(arg(a, 1)?)?.into(),
                            port(arg(a, 2)?)?,
                            if name == "net.connect" {
                                timeout(arg(a, 3)?)?
                            } else {
                                Ok(None)
                            },
                        )
                    };
                    if !canonical_address(&host) {
                        Err(NetFault::InvalidInput)
                    } else if let Err(e) = timeout {
                        Err(e)
                    } else if let Some(rule) = ruleset.iter().find(|r| {
                        r.address == host
                            && r.service == port.to_string()
                            && (name == "net.resolve" || r.operation != "listen")
                    }) {
                        if let Some(fault) = cancellation
                            .map(Fault::Canceled)
                            .or_else(|| self.fault(name))
                        {
                            break 'operation Err(injected(fault)?);
                        }
                        let peer_idx = self.net_worlds.get(&world).and_then(|w| {
                            w.peers
                                .iter()
                                .position(|p| p.address == host && p.port == port)
                        });
                        if let Some(i) = peer_idx {
                            let w = self.net_worlds.get_mut(&world).unwrap();
                            let p = &w.peers[i];
                            if name == "net.resolve" {
                                Ok(collection::sequence(
                                    BuiltinType::List,
                                    p.resolves_to
                                        .iter()
                                        .map(|a| addr_value(a.parse().unwrap(), port)),
                                ))
                            } else {
                                let entry = p
                                    .connections
                                    .get(w.next[i])
                                    .cloned()
                                    .unwrap_or(NetConnection::Refused);
                                w.next[i] += 1;
                                match entry {
                                    NetConnection::Refused => Err(NetFault::ConnectionRefused),
                                    NetConnection::Unanswered => {
                                        Err(if timeout.unwrap().is_some() {
                                            NetFault::TimedOut
                                        } else {
                                            NetFault::Blocked
                                        })
                                    }
                                    NetConnection::Open(steps) => {
                                        let tls = p.tls.clone();
                                        let stream = self.handle("Stream", Grant::Stream { net });
                                        let Value::Handle { id, .. } = &stream else {
                                            unreachable!()
                                        };
                                        self.net_streams.insert(
                                            *id,
                                            ScriptStream::new(
                                                net,
                                                host,
                                                port,
                                                tls,
                                                rule.operation == "connect_tls",
                                                steps,
                                            ),
                                        );
                                        Ok(stream)
                                    }
                                }
                            }
                        } else {
                            Err(if name == "net.resolve" {
                                NetFault::UnknownHost
                            } else {
                                NetFault::ConnectionRefused
                            })
                        }
                    } else {
                        Err(NetFault::NotGranted)
                    }
                }
                "net.listen" => {
                    let f = fields(arg(a, 1)?, "SocketAddr", 2)?;
                    let host = ip(f[0])?.to_string();
                    let service = port(f[1])?.to_string();
                    if !ruleset.iter().any(|r| {
                        r.address == host && r.service == service && r.operation == "listen"
                    }) {
                        Err(NetFault::NotGranted)
                    } else {
                        return Err(Error::Unavailable(
                            "scripted listeners have no incoming peers".into(),
                        ));
                    }
                }
                "net.accept" | "net.local_addr" => {
                    return Err(Error::Unavailable(
                        "scripted listeners have no incoming peers".into(),
                    ));
                }
                _ => {
                    let id = self.stream_id(net, arg(a, 1)?)?;
                    if name == "net.close" {
                        self.record("Net", name, id.to_le_bytes().to_vec(), vec![], cx)?;
                        self.net_streams.remove(&id);
                        self.grants.remove(&id);
                        return Ok(OperationResult::returned(Value::Unit).transferring([1]));
                    }
                    if name == "tls.close" && !self.net_streams[&id].tls {
                        self.net_streams.remove(&id);
                        self.grants.remove(&id);
                        break 'operation Ok(Value::Unit);
                    }
                    if (name == "net.read" && index(arg(a, 3)?)? == 0)
                        || (name == "net.read_into"
                            && collection::elements(arg(a, 2)?, BuiltinType::Inline)?.is_empty())
                    {
                        break 'operation Err(NetFault::InvalidInput);
                    }
                    if matches!(name, "net.set_read_timeout" | "net.set_write_timeout")
                        && let Err(e) = timeout(arg(a, 2)?)?
                    {
                        break 'operation Err(e);
                    }
                    let access = {
                        let s = self.net_streams.get_mut(&id).unwrap();
                        if matches!(
                            name,
                            "tls.connect"
                                | "net.set_read_timeout"
                                | "net.set_write_timeout"
                                | "net.peer_addr"
                        ) {
                            // Owner 385145: invalid repeated upgrades precede
                            // grant-name checks and provider/fault inputs.
                            if name == "tls.connect" && s.tls {
                                Err(NetFault::InvalidInput)
                            } else if name == "tls.connect"
                                && s.requires_tls
                                && text(arg(a, 2)?)? != s.host
                            {
                                Err(NetFault::NotGranted)
                            } else {
                                s.advance();
                                if matches!(s.steps.front(), Some(NetStep::Reset)) {
                                    Err(NetFault::ConnectionReset)
                                } else {
                                    Ok(())
                                }
                            }
                        } else {
                            s.check().and_then(|()| {
                                if matches!(name, "net.write" | "net.write_all") && s.write_closed {
                                    Err(NetFault::BrokenPipe)
                                } else {
                                    Ok(())
                                }
                            })
                        }
                    };
                    let blocking = matches!(
                        name,
                        "net.read"
                            | "net.read_into"
                            | "net.write"
                            | "net.write_all"
                            | "tls.connect"
                            | "tls.close"
                    );
                    let fault = if access.is_ok() && blocking {
                        cancellation
                            .map(Fault::Canceled)
                            .or_else(|| self.fault(name))
                    } else {
                        None
                    };
                    if let Err(e) = access {
                        if matches!(name, "tls.connect" | "tls.close") {
                            self.net_streams.remove(&id);
                            self.grants.remove(&id);
                        }
                        break 'operation Err(e);
                    }
                    if let Some(fault) = fault {
                        if let Fault::Fault { partial, .. } = &fault
                            && *partial > 0
                            && matches!(name, "net.write" | "net.write_all")
                        {
                            let data = bytes(arg(a, 2)?)?;
                            let s = self.net_streams.get_mut(&id).unwrap();
                            s.write(&data[..data.len().min(*partial)])
                                .map_err(|_| invalid("fault write failed after preflight"))?;
                        }
                        if matches!(name, "tls.connect" | "tls.close") {
                            self.net_streams.remove(&id);
                            self.grants.remove(&id);
                        }
                        break 'operation Err(injected(fault)?);
                    }
                    let s = self.net_streams.get_mut(&id).unwrap();
                    match name {
                        "tls.connect" => {
                            let server_name = text(arg(a, 2)?)?;
                            let outcome = if s.requires_tls && server_name != s.host {
                                Err(NetFault::NotGranted)
                            } else if matches!(s.steps.front(), Some(NetStep::Stall)) {
                                Err(if s.read_timeout.is_some() {
                                    NetFault::TimedOut
                                } else {
                                    NetFault::Blocked
                                })
                            } else {
                                match &s.peer {
                                    PeerTls::Certificate(names)
                                        if names.iter().any(|n| n == server_name) =>
                                    {
                                        s.tls = true;
                                        Ok(arg(a, 1)?.clone())
                                    }
                                    PeerTls::Certificate(_) => {
                                        Err(NetFault::Tls(TlsFailure::NameMismatch))
                                    }
                                    PeerTls::Plain => {
                                        Err(NetFault::Tls(TlsFailure::HandshakeFailure))
                                    }
                                    PeerTls::Fails(failure) => Err(NetFault::Tls(*failure)),
                                }
                            };
                            if outcome.as_ref().is_err_and(|e| *e != NetFault::Blocked) {
                                self.net_streams.remove(&id);
                                self.grants.remove(&id);
                            }
                            outcome
                        }
                        "tls.close" => {
                            let outcome = s.check().map(|()| Value::Unit);
                            self.net_streams.remove(&id);
                            self.grants.remove(&id);
                            outcome
                        }
                        "net.read" | "net.read_into" => {
                            let max = if name == "net.read" {
                                index(arg(a, 3)?)?
                            } else {
                                collection::elements(arg(a, 2)?, BuiltinType::Inline)?.len()
                            };
                            match s.read(max) {
                                Ok(bytes) => {
                                    let mut buffer = arg(a, 2)?.clone();
                                    let Value::Sequence { elements, .. } = &mut buffer else {
                                        return Err(invalid("read buffer sequence"));
                                    };
                                    let values = bytes.iter().map(|b| {
                                        Some(Value::Integer(
                                            Integer::new(Primitive::U8, (*b).into()).unwrap(),
                                        ))
                                    });
                                    if name == "net.read" {
                                        elements.extend(values);
                                    } else {
                                        for (slot, value) in elements.iter_mut().zip(values) {
                                            *slot = value;
                                        }
                                    }
                                    updates.push((2, buffer));
                                    recorded = Some(encode_value(&ok(Value::Bytes(bytes.clone()))));
                                    Ok(u64_value(bytes.len() as u64))
                                }
                                Err(e) => Err(e),
                            }
                        }
                        "net.write" | "net.write_all" => s.write(bytes(arg(a, 2)?)?).map(|n| {
                            if name == "net.write" {
                                u64_value(n)
                            } else {
                                Value::Unit
                            }
                        }),
                        "net.set_read_timeout" | "net.set_write_timeout" => {
                            match timeout(arg(a, 2)?)? {
                                Err(e) => Err(e),
                                Ok(t) => {
                                    if name == "net.set_read_timeout" {
                                        s.read_timeout = t
                                    } else {
                                        s.write_timeout = t
                                    };
                                    Ok(Value::Unit)
                                }
                            }
                        }
                        "net.shutdown" => {
                            let (tag, f) = variant_fields(arg(a, 2)?)?;
                            if tag > 2 || !f.is_empty() {
                                return Err(invalid("Shutdown representation"));
                            }
                            s.check().map(|()| {
                                if tag != 1 {
                                    s.read_closed = true;
                                }
                                if tag != 0 {
                                    s.write_closed = true;
                                }
                                Value::Unit
                            })
                        }
                        "net.peer_addr" => {
                            let ip = s
                                .host
                                .parse()
                                .ok()
                                .or_else(|| {
                                    self.net_worlds[&world]
                                        .peers
                                        .iter()
                                        .find(|p| p.address == s.host && p.port == s.port)
                                        .and_then(|p| p.resolves_to.first()?.parse().ok())
                                })
                                .ok_or(NetFault::UnknownHost);
                            ip.map(|ip| addr_value(ip, s.port))
                        }
                        _ => return Err(invalid("unknown network operation")),
                    }
                }
            }
        };
        let value = match result(outcome) {
            Ok(v) => v,
            Err(NetFault::Blocked) => {
                let anchor = cx.anchor().cloned().unwrap_or(crate::observation::Anchor {
                    definition: call.path.clone(),
                    body: hirsel_tir::BodyId(0),
                    cleanup: None,
                    site: call.site,
                });
                let target = if matches!(name, "net.connect" | "net.connect_addr") {
                    net
                } else {
                    self.stream_id(net, arg(a, 1)?)?
                };
                return Err(Error::Blocked(vec![crate::BlockedWait {
                    spawn_path: vec![],
                    anchor,
                    kind: name.into(),
                    targets: vec![target],
                }]));
            }
            Err(_) => unreachable!(),
        };
        let input = if name == "tls.connect" {
            let (tag, payload) = variant_fields(arg(a, 3)?)?;
            let digest = match (tag, payload) {
                (0, []) => variant(0, vec![Value::String("webpki-roots@1.0.9".into())]),
                (1, [Some(pem)]) => {
                    variant(1, vec![Value::Bytes(Sha256::digest(bytes(pem)?).to_vec())])
                }
                _ => return Err(invalid("TlsRoots representation")),
            };
            encode_arguments(&[a[0].clone(), a[1].clone(), a[2].clone(), digest])
        } else if name == "net.read" || name == "net.read_into" {
            let max = if name == "net.read" {
                arg(a, 3)?.clone()
            } else {
                u64_value(collection::elements(arg(a, 2)?, BuiltinType::Inline)?.len() as u64)
            };
            encode_arguments(&[a[0].clone(), a[1].clone(), max])
        } else {
            encode_arguments(a)
        };
        self.record(
            "Net",
            if name == "net.read_into" {
                "net.read"
            } else {
                name
            },
            input,
            recorded.unwrap_or_else(|| encode_value(&value)),
            cx,
        )?;
        let mut result = OperationResult::returned(value);
        if let OperationResult::Complete {
            arguments,
            consumed,
            ..
        } = &mut result
        {
            *arguments = updates;
            if matches!(name, "net.close" | "tls.connect" | "tls.close") {
                consumed.push(1);
            }
        }
        Ok(result)
    }
}
fn validate_peers(peers: &[NetPeer]) -> Result<(), Error> {
    let mut seen = BTreeMap::new();
    for (index, peer) in peers.iter().enumerate() {
        if let Some(first) = seen.insert((&peer.address, peer.port), index) {
            return Err(invalid(format!(
                "duplicate address/port at entries {first} and {index}"
            )));
        }
        if !canonical_address(&peer.address)
            || peer.resolves_to.iter().any(|a| {
                a.parse::<IpAddr>().is_err()
                    || a.parse::<IpAddr>().is_ok_and(|ip| ip.to_string() != *a)
            })
        {
            return Err(invalid("noncanonical simulation peer address"));
        }
    }
    Ok(())
}
pub(super) fn configured_world(grant: Option<&NetGrant>) -> Result<BTreeMap<u64, World>, Error> {
    if let Some(grant) = grant {
        if let Some(peers) = &grant.peers {
            validate_peers(peers)?;
        }
        if grant.rules.iter().any(|r| {
            !canonical_address(&r.address)
                || r.service.parse::<u16>().is_err()
                || r.service
                    .parse::<u16>()
                    .is_ok_and(|p| p.to_string() != r.service)
                || !matches!(r.operation.as_str(), "connect" | "connect_tls" | "listen")
        }) {
            return Err(invalid("invalid simulation network rules"));
        }
    }
    match grant {
        Some(NetGrant {
            provider,
            peers: Some(peers),
            ..
        }) if ProviderContract::parse(provider) == Some(ProviderContract::NetScript) => {
            Ok(BTreeMap::from([(0, World::new(peers.clone()))]))
        }
        None => Ok(BTreeMap::from([(0, World::new(vec![]))])),
        _ => Err(Error::Unavailable(format!(
            "interpreter requires {}",
            ProviderContract::NetScript.name()
        ))),
    }
}
#[cfg(test)]
#[path = "net_tests.rs"]
mod tests;

/// A suspended Io call retains its typed snapshot; the evaluator retains owners
/// and mutable loans until this operation completes or the run is abandoned.
pub(super) struct NetworkWait {
    name: String,
    path: hirsel_syntax::DefPath,
    arguments: Vec<Value>,
    generics: Vec<hirsel_types::GenericArgument>,
    types: hirsel_types::TypeStore,
    result_type: hirsel_types::TypeId,
    site: hirsel_span::Span,
}
impl super::Library {
    pub(super) fn network_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        self.tasks.yield_now = true;
        let saved = (!context.configuration.inputs.is_empty()).then(|| self.simulation.clone());
        let result = self
            .simulation
            .net_call(name, call, context, self.tasks.canceled());
        match result {
            Err(Error::Blocked(mut waits)) => {
                let token = self.next_callback;
                self.next_callback += 1;
                self.network_waits.insert(
                    token,
                    NetworkWait {
                        name: name.into(),
                        path: call.path.clone(),
                        arguments: call.arguments.to_vec(),
                        generics: call.generics.to_vec(),
                        types: call.types.clone(),
                        result_type: call.result_type,
                        site: call.site,
                    },
                );
                let mut wait = waits.pop().ok_or_else(|| invalid("network wait missing"))?;
                wait.spawn_path = self.tasks.path();
                Ok(OperationResult::Park { token, wait })
            }
            Err(error) => {
                if let Some(saved) = saved {
                    self.simulation = saved;
                }
                Err(error)
            }
            Ok(result) => Ok(result),
        }
    }
    pub(super) fn poll_network(
        &mut self,
        token: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let wait = &self.network_waits[&token];
        // An Unanswered scripted connection has no readiness source. Polling
        // must not issue a new connect and consume the next scripted attempt.
        if matches!(wait.name.as_str(), "net.connect" | "net.connect_addr")
            && self.tasks.canceled().is_none()
        {
            return Ok(None);
        }
        let saved = (!context.configuration.inputs.is_empty()).then(|| self.simulation.clone());
        let result = self.simulation.net_call(
            &wait.name,
            &OperationCall {
                path: &wait.path,
                arguments: &wait.arguments,
                generics: &wait.generics,
                types: &wait.types,
                result_type: wait.result_type,
                site: wait.site,
            },
            context,
            self.tasks.canceled(),
        );
        match result {
            Err(Error::Blocked(_)) => Ok(None),
            Err(error) => {
                if let Some(saved) = saved {
                    self.simulation = saved;
                }
                Err(error)
            }
            Ok(result) => {
                self.network_waits.remove(&token);
                Ok(Some(result))
            }
        }
    }
}
