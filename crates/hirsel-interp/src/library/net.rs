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
    NetClient, NetConnection, NetGrant, NetListener, NetPeer, NetRule, NetStep, PeerTls,
    ProviderContract, TlsFailure, canonical_net_address as canonical_address,
};
use hirsel_types::{BuiltinType, Primitive};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    net::{IpAddr, SocketAddr},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct World {
    peers: Vec<NetPeer>,
    next_port: u32,
    next: Vec<usize>,
    listeners: Vec<NetListener>,
    /// The next client of each scripted listener address.
    accepted: Vec<usize>,
}
impl World {
    pub fn new(peers: Vec<NetPeer>, listeners: Vec<NetListener>) -> Self {
        Self {
            next: vec![0; peers.len()],
            peers,
            next_port: 49152,
            accepted: vec![0; listeners.len()],
            listeners,
        }
    }
}
/// A bound datagram socket with independent scripted peers and queued packets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ScriptUdp {
    pub net: u64,
    world: u64,
    local: SocketAddr,
    peers: Vec<(SocketAddr, VecDeque<NetStep>)>,
    received: VecDeque<(Vec<u8>, SocketAddr)>,
}
/// An open listener: its Net, world and address, an IP literal and port or a socket
/// path with port 0.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ScriptListener {
    pub net: u64,
    world: u64,
    address: (String, u16),
    /// Opened under a ListenTls rule alone: its streams carry no plaintext before tls.accept.
    tls_only: bool,
}
impl ScriptListener {
    /// `net.local_addr`: a Unix listener's is `0.0.0.0` with port 0.
    fn local(&self) -> SocketAddr {
        self.address.0.parse().map_or(
            SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0),
            |ip| SocketAddr::new(ip, self.address.1),
        )
    }
    fn name(&self) -> String {
        hirsel_run_config::listener_name(&self.address.0, self.address.1)
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
    /// An accepted stream's client index and listen address: its `Expect`
    /// mismatches fail the test instead of resetting. Its `peer` is the client's TLS.
    client: Option<(usize, String)>,
    /// What `tls.peer` returns: the verified chain a `tls.accept` client presented.
    chain: Option<Vec<u8>>,
    mismatch: Option<String>,
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
    ConnectionAborted,
    AddrInUse,
    AddrNotAvailable,
    /// A scripted client's Expect failed: the Test report of the LANG-9 failure.
    Mismatch(String),
    /// An accepted stream closed before its client's Expect was met (LANG-9).
    Unmet(String),
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
            client: None,
            chain: None,
            mismatch: None,
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
                        if let Some((client, listener)) = &self.client {
                            let wrote: Vec<u8> =
                                self.written.iter().take(b.len()).copied().collect();
                            self.mismatch = Some(hirsel_run_config::net_client_mismatch(
                                *client, listener, b, &wrote,
                            ));
                        }
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
    /// The Test report of closing an accepted stream whose client still expects
    /// bytes before any Reset step.
    fn unmet(&mut self) -> Option<String> {
        let (client, listener) = self.client.clone()?;
        self.advance();
        let expected = self
            .steps
            .iter()
            .take_while(|step| !matches!(step, NetStep::Reset))
            .find_map(|step| match step {
                NetStep::Expect(b) => Some(b),
                _ => None,
            })?;
        let wrote: Vec<u8> = self.written.iter().copied().collect();
        Some(hirsel_run_config::net_client_unmet(
            client, &listener, expected, &wrote,
        ))
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
        if let Some(report) = self.mismatch.take() {
            return Err(NetFault::Mismatch(report));
        }
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
fn steps(v: &Value) -> Result<Vec<NetStep>, Error> {
    collection::elements(v, BuiltinType::List)?
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
        .collect()
}
fn socket_addr(v: &Value) -> Result<SocketAddr, Error> {
    let f = fields(v, "SocketAddr", 2)?;
    Ok(SocketAddr::new(ip(f[0])?, port(f[1])?))
}
fn listener(v: &Value) -> Result<NetListener, Error> {
    let f = fields(v, "NetListener", 3)?;
    Ok(NetListener {
        address: text(f[0])?.to_owned(),
        port: port(f[1])?,
        clients: collection::elements(f[2], BuiltinType::List)?
            .iter()
            .map(|c| {
                let f = fields(c, "NetClient", 3)?;
                let from = socket_addr(f[0])?;
                Ok(NetClient {
                    address: from.ip().to_string(),
                    port: from.port(),
                    tls: peer_tls(f[1])?,
                    steps: steps(f[2])?,
                })
            })
            .collect::<Result<_, Error>>()?,
    })
}
fn peer_tls(v: &Value) -> Result<PeerTls, Error> {
    let (tag, tls) = variant_fields(v)?;
    Ok(match (tag, tls) {
        (0, []) => PeerTls::Plain,
        (1, [Some(names)]) => PeerTls::Certificate(
            collection::elements(names, BuiltinType::List)?
                .iter()
                .map(|n| text(n).map(str::to_owned))
                .collect::<Result<_, _>>()?,
        ),
        (2, [Some(failure)]) => PeerTls::Fails(tls_failure(failure)?),
        (3, [Some(names), Some(chain)]) => PeerTls::Presents(
            collection::elements(names, BuiltinType::List)?
                .iter()
                .map(|n| text(n).map(str::to_owned))
                .collect::<Result<_, _>>()?,
            bytes(chain)?.to_vec(),
        ),
        _ => return Err(invalid("PeerTls representation")),
    })
}
fn peer(v: &Value) -> Result<NetPeer, Error> {
    let f = fields(v, "NetPeer", 5)?;
    let tls = peer_tls(f[3])?;
    let mut connections = vec![];
    for c in collection::elements(f[4], BuiltinType::List)? {
        let (tag, params) = variant_fields(&c)?;
        connections.push(match (tag, params) {
            (0, []) => NetConnection::Refused,
            (1, []) => NetConnection::Unanswered,
            (2, [Some(s)]) => NetConnection::Open(steps(s)?),
            _ => return Err(invalid("NetConnection representation")),
        });
    }
    let peer = NetPeer {
        address: text(f[0])?.to_owned(),
        port: port(f[1])?,
        resolves_to: collection::elements(f[2], BuiltinType::List)?
            .iter()
            .map(|v| ip(v).map(|ip| ip.to_string()))
            .collect::<Result<_, _>>()?,
        tls,
        connections,
    };
    hirsel_run_config::check_peer_address(&peer)
        .map_err(|e| invalid(format!("test.net peer: {e}")))?;
    Ok(peer)
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
/// RunConfig operation names, in `NetOperation` variant order.
const OPERATIONS: [&str; 9] = [
    "connect",
    "connect_tls",
    "listen",
    "udp_bind",
    "udp_peer",
    "listen_tls",
    "unix_connect",
    "unix_listen",
    "unix_listen_remove_stale",
];
fn rules(v: &Value) -> Result<Vec<NetRule>, Error> {
    collection::elements(v, BuiltinType::List)?
        .iter()
        .map(|v| {
            let f = fields(v, "NetRule", 3)?;
            let (op, payload) = variant_fields(f[2])?;
            if !payload.is_empty() || op as usize >= OPERATIONS.len() {
                return Err(invalid("NetOperation representation"));
            }
            Ok(NetRule {
                address: text(f[0])?.into(),
                service: port(f[1])?.to_string(),
                operation: OPERATIONS[op as usize].into(),
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
        NetFault::Blocked | NetFault::Mismatch(_) | NetFault::Unmet(_) => Err(e),
        NetFault::Canceled(cause) => Ok(error(variant(1, vec![variant(cause, vec![])]))),
        NetFault::Fault(code) => Ok(error(variant(17, vec![super::u32_value(code)]))),
        NetFault::NotGranted => Ok(error(variant(0, vec![]))),
        NetFault::InvalidInput => Ok(error(variant(2, vec![]))),
        NetFault::UnknownHost => Ok(error(variant(3, vec![]))),
        NetFault::ConnectionRefused => Ok(error(variant(5, vec![]))),
        NetFault::ConnectionReset => Ok(error(variant(6, vec![]))),
        NetFault::ConnectionAborted => Ok(error(variant(7, vec![]))),
        NetFault::AddrNotAvailable => Ok(error(variant(10, vec![]))),
        NetFault::AddrInUse => Ok(error(variant(9, vec![]))),
        NetFault::BrokenPipe => Ok(error(variant(14, vec![]))),
        NetFault::TimedOut => Ok(error(variant(15, vec![]))),
        NetFault::Tls(failure) => Ok(error(variant(16, vec![tls_failure_value(failure)]))),
    })
}
impl Simulation {
    /// Close a scripted stream or listener: the Test report when a client it
    /// leaves still expects bytes (docs/spec/core-library.md, scripted peers).
    pub(super) fn close_net(&mut self, id: u64) -> Option<String> {
        self.grants.remove(&id);
        self.udp_sockets.remove(&id);
        if let Some(mut stream) = self.net_streams.remove(&id) {
            return stream.unmet();
        }
        let listener = self.net_listeners.remove(&id)?;
        let world = &self.net_worlds[&listener.world];
        let i = world
            .listeners
            .iter()
            .position(|l| (&l.address, l.port) == (&listener.address.0, listener.address.1))?;
        let client =
            hirsel_run_config::first_expecting_client(&world.listeners[i], world.accepted[i])?;
        Some(hirsel_run_config::net_client_unaccepted(
            client,
            &listener.name(),
        ))
    }
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
    fn listener_id(&self, n: u64, v: &Value) -> Result<u64, Error> {
        let id = self.id(v, "Listener")?;
        if self.net_listeners.get(&id).is_none_or(|l| l.net != n) {
            return Err(invalid("wrong listener Net"));
        }
        Ok(id)
    }
    fn stream_id(&self, n: u64, v: &Value) -> Result<u64, Error> {
        let id = self.id(v, "Stream")?;
        if self.net_streams.get(&id).is_none_or(|s| s.net != n) {
            return Err(invalid("wrong stream Net"));
        }
        Ok(id)
    }
    /// Take a scripted peer's next connection entry for a connect to `host`/`port`.
    fn open_peer(
        &mut self,
        net: u64,
        world: u64,
        host: String,
        port: u16,
        timeout: Option<u64>,
        requires_tls: bool,
    ) -> NetResult<Value> {
        let Some(w) = self.net_worlds.get_mut(&world) else {
            return Err(NetFault::ConnectionRefused);
        };
        let Some(i) = w
            .peers
            .iter()
            .position(|p| p.address == host && p.port == port)
        else {
            return Err(NetFault::ConnectionRefused);
        };
        let p = &w.peers[i];
        let entry = p
            .connections
            .get(w.next[i])
            .cloned()
            .unwrap_or(NetConnection::Refused);
        let tls = p.tls.clone();
        w.next[i] += 1;
        match entry {
            NetConnection::Refused => Err(NetFault::ConnectionRefused),
            NetConnection::Unanswered => Err(if timeout.is_some() {
                NetFault::TimedOut
            } else {
                NetFault::Blocked
            }),
            NetConnection::Open(steps) => {
                let stream = self.handle("Stream", Grant::Stream { net });
                let Value::Handle { id, .. } = &stream else {
                    unreachable!()
                };
                self.net_streams.insert(
                    *id,
                    ScriptStream::new(net, host, port, tls, requires_tls, steps),
                );
                Ok(stream)
            }
        }
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
            let listeners = collection::elements(arg(a, 2)?, BuiltinType::List)?
                .iter()
                .map(listener)
                .collect::<Result<Vec<_>, _>>()?;
            validate_peers(&peers)?;
            hirsel_run_config::check_net_listeners(&listeners).map_err(invalid)?;
            let rules = peers
                .iter()
                .map(|p| NetRule {
                    address: p.address.clone(),
                    service: p.port.to_string(),
                    operation: if hirsel_run_config::canonical_unix_path(&p.address) {
                        "unix_connect"
                    } else if matches!(p.tls, PeerTls::Plain) {
                        "connect"
                    } else {
                        "connect_tls"
                    }
                    .into(),
                })
                .chain(listeners.iter().map(|l| {
                    NetRule {
                        address: l.address.clone(),
                        service: l.port.to_string(),
                        operation: if hirsel_run_config::canonical_unix_path(&l.address) {
                            "unix_listen"
                        } else {
                            "listen"
                        }
                        .into(),
                    }
                }))
                .chain(
                    peers
                        .iter()
                        .filter(|p| {
                            matches!(p.tls, PeerTls::Plain) && p.address.parse::<IpAddr>().is_ok()
                        })
                        .map(|p| NetRule {
                            address: p.address.clone(),
                            service: p.port.to_string(),
                            operation: "udp_peer".into(),
                        }),
                )
                .chain(
                    listeners
                        .iter()
                        .filter(|l| l.address.parse::<IpAddr>().is_ok())
                        .map(|l| NetRule {
                            address: l.address.clone(),
                            service: l.port.to_string(),
                            operation: "udp_bind".into(),
                        }),
                )
                .collect();
            let world = self.next_id;
            self.net_worlds.insert(world, World::new(peers, listeners));
            let value = self.handle("Net", Grant::Net { world, rules });
            self.record("Test", name, encode_arguments(a), encode_value(&value), cx)?;
            return Ok(OperationResult::returned(value));
        }
        let (net, world, ruleset) = self.net_grant(arg(a, 0)?)?;
        let mut updates = vec![];
        let mut recorded = None;
        let outcome = 'operation: {
            match name {
                "net.udp_bind" => {
                    let f = fields(arg(a, 1)?, "SocketAddr", 2)?;
                    let mut local = SocketAddr::new(ip(f[0])?, port(f[1])?);
                    if !ruleset.iter().any(|r| {
                        r.operation == "udp_bind"
                            && r.address == local.ip().to_string()
                            && r.service == local.port().to_string()
                    }) {
                        Err(NetFault::NotGranted)
                    } else if let Some(cause) = cancellation {
                        Err(NetFault::Canceled(cause))
                    } else {
                        if local.port() == 0 {
                            let w = self
                                .net_worlds
                                .get_mut(&world)
                                .ok_or_else(|| invalid("UDP world"))?;
                            while w.next_port <= u16::MAX as u32
                                && self.udp_sockets.values().any(|s| {
                                    s.world == world
                                        && s.local
                                            == SocketAddr::new(local.ip(), w.next_port as u16)
                                })
                            {
                                w.next_port += 1;
                            }
                            if w.next_port > u16::MAX as u32 {
                                break 'operation Err(NetFault::AddrNotAvailable);
                            }
                            local.set_port(w.next_port as u16);
                            w.next_port += 1;
                        }
                        if self
                            .udp_sockets
                            .values()
                            .any(|s| s.world == world && s.local == local)
                        {
                            Err(NetFault::AddrInUse)
                        } else {
                            let peers = self.net_worlds[&world]
                                .peers
                                .iter()
                                .filter_map(|p| {
                                    if !matches!(p.tls, PeerTls::Plain) {
                                        return None;
                                    }
                                    let peer = SocketAddr::new(p.address.parse().ok()?, p.port);
                                    let steps = match p.connections.first()? {
                                        NetConnection::Open(s) => s.clone(),
                                        NetConnection::Refused => vec![NetStep::Reset],
                                        NetConnection::Unanswered => vec![NetStep::Stall],
                                    };
                                    Some((peer, steps.into()))
                                })
                                .collect();
                            let socket = self.handle("UdpSocket", Grant::UdpSocket { net });
                            let id = self.id(&socket, "UdpSocket")?;
                            self.udp_sockets.insert(
                                id,
                                ScriptUdp {
                                    net,
                                    world,
                                    local,
                                    peers,
                                    received: VecDeque::new(),
                                },
                            );
                            Ok(socket)
                        }
                    }
                }
                "udp.send_to" | "udp.recv_from" | "udp.close" => {
                    let id = self.id(arg(a, 1)?, "UdpSocket")?;
                    if self.udp_sockets.get(&id).is_none_or(|s| s.net != net) {
                        return Err(invalid("wrong UDP Net"));
                    }
                    if name == "udp.close" {
                        self.close_net(id);
                        self.record(
                            "Net",
                            name,
                            encode_arguments(a),
                            encode_value(&Value::Unit),
                            cx,
                        )?;
                        return Ok(OperationResult::returned(Value::Unit).transferring([1]));
                    } else if name == "udp.send_to" {
                        let data = bytes(arg(a, 2)?)?;
                        let f = fields(arg(a, 3)?, "SocketAddr", 2)?;
                        let peer = SocketAddr::new(ip(f[0])?, port(f[1])?);
                        if data.len() > 65507 {
                            Err(NetFault::InvalidInput)
                        } else if !ruleset.iter().any(|r| {
                            r.operation == "udp_peer"
                                && r.address == peer.ip().to_string()
                                && r.service == peer.port().to_string()
                        }) {
                            Err(NetFault::NotGranted)
                        } else if let Some(cause) = cancellation {
                            Err(NetFault::Canceled(cause))
                        } else {
                            let local = self.udp_sockets[&id].local;
                            if let Some(target) = self
                                .udp_sockets
                                .values_mut()
                                .find(|s| s.world == world && s.local == peer)
                            {
                                target.received.push_back((data.to_vec(), local));
                            } else if let Some((_, steps)) = self
                                .udp_sockets
                                .get_mut(&id)
                                .unwrap()
                                .peers
                                .iter_mut()
                                .find(|(p, _)| *p == peer)
                            {
                                match steps.front() {
                                    Some(NetStep::Expect(expected)) if expected == data => {
                                        steps.pop_front();
                                    }
                                    Some(NetStep::Expect(_) | NetStep::Reset) => {
                                        break 'operation Err(NetFault::ConnectionReset);
                                    }
                                    _ => {}
                                }
                            }
                            Ok(u64_value(data.len() as u64))
                        }
                    } else {
                        collection::elements(arg(a, 2)?, BuiltinType::List)?;
                        let max = index(arg(a, 3)?)?;
                        if max == 0 {
                            Err(NetFault::InvalidInput)
                        } else if let Some(cause) = cancellation {
                            Err(NetFault::Canceled(cause))
                        } else {
                            loop {
                                let socket = self.udp_sockets.get_mut(&id).unwrap();
                                let datagram = socket.received.pop_front().map(Ok).or_else(|| {
                                    socket.peers.iter_mut().find_map(|(peer, steps)| {
                                        match steps.front() {
                                            Some(NetStep::Send(_)) => {
                                                let Some(NetStep::Send(bytes)) = steps.pop_front()
                                                else {
                                                    unreachable!()
                                                };
                                                Some(Ok((bytes, *peer)))
                                            }
                                            Some(NetStep::Reset) => {
                                                Some(Err(NetFault::ConnectionReset))
                                            }
                                            _ => None,
                                        }
                                    })
                                });
                                let Some(datagram) = datagram else {
                                    break Err(NetFault::Blocked);
                                };
                                let (mut data, peer) = match datagram {
                                    Ok(d) => d,
                                    Err(e) => break Err(e),
                                };
                                if !ruleset.iter().any(|r| {
                                    r.operation == "udp_peer"
                                        && r.address == peer.ip().to_string()
                                        && r.service == peer.port().to_string()
                                }) {
                                    continue;
                                }
                                let truncated = data.len() > max;
                                data.truncate(max);
                                let count = data.len() as u64;
                                let mut buffer = arg(a, 2)?.clone();
                                let Value::Sequence { elements, .. } = &mut buffer else {
                                    return Err(invalid("UDP buffer"));
                                };
                                elements.extend(data.iter().map(|b| {
                                    Some(Value::Integer(
                                        Integer::new(Primitive::U8, *b as u128).unwrap(),
                                    ))
                                }));
                                updates.push((2, buffer));
                                let received = Value::Record {
                                    definition: hirsel_syntax::DefPath::core("UdpReceived"),
                                    fields: vec![
                                        Some(u64_value(count)),
                                        Some(addr_value(peer.ip(), peer.port())),
                                        Some(Value::Bool(truncated)),
                                    ],
                                };
                                recorded = Some(encode_arguments(&[
                                    ok(received.clone()),
                                    Value::Bytes(data),
                                ]));
                                break Ok(received);
                            }
                        }
                    }
                }
                "net.narrow" => {
                    let rules = rules(arg(a, 1)?)?;
                    if rules.iter().any(|r| {
                        !canonical_address(&r.address)
                            && !hirsel_run_config::canonical_unix_path(&r.address)
                    }) {
                        Err(NetFault::InvalidInput)
                    } else if !rules.iter().all(|r| {
                        ruleset.iter().any(|p| {
                            p.address == r.address
                                && p.service == r.service
                                && (p.operation == r.operation
                                    || p.operation == "connect" && r.operation == "connect_tls"
                                    || p.operation == "listen" && r.operation == "listen_tls"
                                    || p.operation == "unix_listen_remove_stale"
                                        && r.operation == "unix_listen")
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
                            && (name == "net.resolve"
                                || matches!(r.operation.as_str(), "connect" | "connect_tls"))
                    }) {
                        if let Some(fault) = cancellation
                            .map(Fault::Canceled)
                            .or_else(|| self.fault(name))
                        {
                            break 'operation Err(injected(fault)?);
                        }
                        if name != "net.resolve" {
                            let requires_tls = rule.operation == "connect_tls";
                            break 'operation self.open_peer(
                                net,
                                world,
                                host,
                                port,
                                timeout.unwrap(),
                                requires_tls,
                            );
                        }
                        match self.net_worlds.get(&world).and_then(|w| {
                            w.peers.iter().find(|p| p.address == host && p.port == port)
                        }) {
                            Some(p) => Ok(collection::sequence(
                                BuiltinType::List,
                                p.resolves_to
                                    .iter()
                                    .map(|a| addr_value(a.parse().unwrap(), port)),
                            )),
                            None => Err(NetFault::UnknownHost),
                        }
                    } else {
                        Err(NetFault::NotGranted)
                    }
                }
                "net.unix_connect" | "net.unix_listen" => {
                    let path = super::dispatch::path_text(arg(a, 1)?)?.to_owned();
                    let listen = name == "net.unix_listen";
                    if !hirsel_run_config::canonical_unix_path(&path) {
                        break 'operation Err(NetFault::InvalidInput);
                    }
                    if !ruleset.iter().any(|r| {
                        r.address == path
                            && r.service == "0"
                            && if listen {
                                matches!(
                                    r.operation.as_str(),
                                    "unix_listen" | "unix_listen_remove_stale"
                                )
                            } else {
                                r.operation == "unix_connect"
                            }
                    }) {
                        break 'operation Err(NetFault::NotGranted);
                    }
                    if !listen {
                        if let Some(fault) = cancellation
                            .map(Fault::Canceled)
                            .or_else(|| self.fault(name))
                        {
                            break 'operation Err(injected(fault)?);
                        }
                        break 'operation self.open_peer(net, world, path, 0, None, false);
                    }
                    // A scripted world has no socket files, so none is ever stale.
                    let address = (path, 0);
                    if self
                        .net_listeners
                        .values()
                        .any(|l| l.world == world && l.address == address)
                    {
                        break 'operation Err(NetFault::AddrInUse);
                    }
                    let listener = self.handle("Listener", Grant::Listener { net });
                    let Value::Handle { id, .. } = &listener else {
                        unreachable!()
                    };
                    self.net_listeners.insert(
                        *id,
                        ScriptListener {
                            net,
                            world,
                            address,
                            tls_only: false,
                        },
                    );
                    Ok(listener)
                }
                "net.listen" => {
                    let mut address = socket_addr(arg(a, 1)?)?;
                    let host = address.ip().to_string();
                    let service = address.port().to_string();
                    // A Listen rule is the wider grant and wins over ListenTls.
                    let granted = |operation: &str| {
                        ruleset.iter().any(|r| {
                            r.address == host && r.service == service && r.operation == operation
                        })
                    };
                    let tls_only = !granted("listen");
                    if tls_only && !granted("listen_tls") {
                        Err(NetFault::NotGranted)
                    } else {
                        if address.port() == 0 {
                            let w = self.net_worlds.get_mut(&world).unwrap();
                            while w.next_port <= u32::from(u16::MAX)
                                && (w.listeners.iter().any(|l| u32::from(l.port) == w.next_port)
                                    || w.peers.iter().any(|p| u32::from(p.port) == w.next_port)
                                    || self.net_listeners.values().any(|l| {
                                        l.world == world && u32::from(l.address.1) == w.next_port
                                    }))
                            {
                                w.next_port += 1;
                            }
                            let Ok(port) = u16::try_from(w.next_port) else {
                                break 'operation Err(NetFault::AddrInUse);
                            };
                            w.next_port += 1;
                            address.set_port(port);
                            if let Some(template) = w
                                .listeners
                                .iter_mut()
                                .find(|l| l.port == 0 && l.address == host)
                            {
                                template.port = port;
                            }
                        }
                        let address = (address.ip().to_string(), address.port());
                        if self
                            .net_listeners
                            .values()
                            .any(|l| l.world == world && l.address == address)
                        {
                            Err(NetFault::AddrInUse)
                        } else {
                            let listener = self.handle("Listener", Grant::Listener { net });
                            let Value::Handle { id, .. } = &listener else {
                                unreachable!()
                            };
                            self.net_listeners.insert(
                                *id,
                                ScriptListener {
                                    net,
                                    world,
                                    address,
                                    tls_only,
                                },
                            );
                            Ok(listener)
                        }
                    }
                }
                "net.local_addr" => {
                    let id = self.listener_id(net, arg(a, 1)?)?;
                    let address = self.net_listeners[&id].local();
                    Ok(addr_value(address.ip(), address.port()))
                }
                "net.accept" => {
                    let id = self.listener_id(net, arg(a, 1)?)?;
                    if let Some(fault) = cancellation
                        .map(Fault::Canceled)
                        .or_else(|| self.fault(name))
                    {
                        break 'operation Err(injected(fault)?);
                    }
                    let listening = &self.net_listeners[&id];
                    let (address, name, tls_only) = (
                        listening.address.clone(),
                        listening.name(),
                        listening.tls_only,
                    );
                    let w = self.net_worlds.get_mut(&world).unwrap();
                    let next = w
                        .listeners
                        .iter()
                        .position(|l| (&l.address, l.port) == (&address.0, address.1))
                        .and_then(|i| {
                            let index = w.accepted[i];
                            let client = w.listeners[i].clients.get(index)?.clone();
                            w.accepted[i] += 1;
                            Some((index, client))
                        });
                    let Some((index, client)) = next else {
                        break 'operation Err(NetFault::ConnectionAborted);
                    };
                    let from = SocketAddr::new(
                        client.address.parse().expect("validated client address"),
                        client.port,
                    );
                    let stream = self.handle("Stream", Grant::Stream { net });
                    let Value::Handle { id: stream_id, .. } = &stream else {
                        unreachable!()
                    };
                    let mut script = ScriptStream::new(
                        net,
                        client.address,
                        client.port,
                        client.tls,
                        tls_only,
                        client.steps,
                    );
                    script.client = Some((index, name));
                    self.net_streams.insert(*stream_id, script);
                    Ok(Value::Tuple(vec![
                        Some(stream),
                        Some(addr_value(from.ip(), from.port())),
                    ]))
                }
                _ => {
                    let id = self.stream_id(net, arg(a, 1)?)?;
                    if name == "net.close" {
                        let report = self.close_net(id);
                        let output = match &report {
                            Some(_) => b"failure:LANG-9".to_vec(),
                            None => vec![],
                        };
                        self.record("Net", name, id.to_le_bytes().to_vec(), output, cx)?;
                        if let Some(report) = report {
                            break 'operation Err(NetFault::Unmet(report));
                        }
                        return Ok(OperationResult::returned(Value::Unit).transferring([1]));
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
                        let s = &self.net_streams[&id];
                        let invalid_tls = match name {
                            "tls.connect" | "tls.connect_as" => {
                                let invalid_roots = match variant_fields(arg(a, 3)?)? {
                                    (0, []) => false,
                                    (1, [Some(pem)]) => {
                                        hirsel_run_config::tls::pem_roots(bytes(pem)?).is_none()
                                    }
                                    _ => return Err(invalid("TlsRoots representation")),
                                };
                                invalid_roots
                                    || hirsel_run_config::tls::server_name(text(arg(a, 2)?)?)
                                        .is_none()
                                    || name == "tls.connect_as" && identity(arg(a, 4)?)?.is_none()
                            }
                            "tls.accept" => server(arg(a, 2)?)?.is_none(),
                            _ => false,
                        };
                        let upgrade =
                            matches!(name, "tls.connect" | "tls.connect_as" | "tls.accept");
                        if upgrade && (s.tls || invalid_tls) {
                            Err(NetFault::InvalidInput)
                        } else if matches!(name, "tls.connect" | "tls.connect_as")
                            && s.requires_tls
                            && (s.client.is_some() || text(arg(a, 2)?)? != s.host)
                            || name == "tls.accept" && s.client.is_none()
                            || s.requires_tls
                                && !s.tls
                                && matches!(
                                    name,
                                    "net.read"
                                        | "net.read_into"
                                        | "net.write"
                                        | "net.write_all"
                                        | "net.shutdown"
                                )
                        {
                            Err(NetFault::NotGranted)
                        } else {
                            Ok(())
                        }
                    };
                    let blocking = matches!(
                        name,
                        "net.read"
                            | "net.read_into"
                            | "net.write"
                            | "net.write_all"
                            | "tls.connect"
                            | "tls.connect_as"
                            | "tls.accept"
                            | "tls.close"
                    );
                    if let Err(e) = access {
                        if matches!(
                            name,
                            "tls.connect" | "tls.connect_as" | "tls.accept" | "tls.close"
                        ) && let Some(report) = self.close_net(id)
                        {
                            break 'operation Err(NetFault::Unmet(report));
                        }
                        break 'operation Err(e);
                    }
                    if let Some(cause) = cancellation.filter(|_| blocking) {
                        if matches!(
                            name,
                            "tls.connect" | "tls.connect_as" | "tls.accept" | "tls.close"
                        ) && let Some(report) = self.close_net(id)
                        {
                            break 'operation Err(NetFault::Unmet(report));
                        }
                        break 'operation Err(NetFault::Canceled(cause));
                    }
                    // Handshake state, not stream input: no step is consulted.
                    if name == "tls.peer" {
                        break 'operation Ok(match &self.net_streams[&id].chain {
                            Some(chain) => super::dispatch::some(Value::Bytes(chain.clone())),
                            None => super::dispatch::none(),
                        });
                    }
                    if name == "tls.close" && !self.net_streams[&id].tls {
                        if let Some(report) = self.close_net(id) {
                            break 'operation Err(NetFault::Unmet(report));
                        }
                        break 'operation Ok(Value::Unit);
                    }
                    let s = self.net_streams.get_mut(&id).unwrap();
                    s.advance();
                    if matches!(s.steps.front(), Some(NetStep::Reset)) {
                        if matches!(
                            name,
                            "tls.connect" | "tls.connect_as" | "tls.accept" | "tls.close"
                        ) && let Some(report) = self.close_net(id)
                        {
                            break 'operation Err(NetFault::Unmet(report));
                        }
                        break 'operation Err(NetFault::ConnectionReset);
                    }
                    if matches!(name, "net.write" | "net.write_all") && s.write_closed {
                        break 'operation Err(NetFault::BrokenPipe);
                    }
                    let fault = blocking.then(|| self.fault(name)).flatten();
                    if let Some(fault) = fault {
                        if let Fault::Fault { partial, .. } = &fault
                            && *partial > 0
                            && matches!(name, "net.write" | "net.write_all")
                        {
                            let data = bytes(arg(a, 2)?)?;
                            let s = self.net_streams.get_mut(&id).unwrap();
                            match s.write(&data[..data.len().min(*partial)]) {
                                Ok(_) => {}
                                Err(e @ NetFault::Mismatch(_)) => break 'operation Err(e),
                                Err(_) => {
                                    return Err(invalid("fault write failed after preflight"));
                                }
                            }
                        }
                        if matches!(
                            name,
                            "tls.connect" | "tls.connect_as" | "tls.accept" | "tls.close"
                        ) && let Some(report) = self.close_net(id)
                        {
                            break 'operation Err(NetFault::Unmet(report));
                        }
                        break 'operation Err(injected(fault)?);
                    }
                    let s = self.net_streams.get_mut(&id).unwrap();
                    match name {
                        // Scripted peers never request a client certificate.
                        "tls.connect" | "tls.connect_as" => {
                            let server_name = text(arg(a, 2)?)?;
                            let outcome = if matches!(s.steps.front(), Some(NetStep::Stall)) {
                                Err(if s.read_timeout.is_some() {
                                    NetFault::TimedOut
                                } else {
                                    NetFault::Blocked
                                })
                            } else if s.client.is_some() {
                                // An accepted stream's client runs no TLS server.
                                Err(NetFault::Tls(TlsFailure::HandshakeFailure))
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
                                    PeerTls::Presents(..) => {
                                        return Err(invalid("a peer's TLS is never Presents"));
                                    }
                                }
                            };
                            if outcome.as_ref().is_err_and(|e| *e != NetFault::Blocked)
                                && let Some(report) = self.close_net(id)
                            {
                                break 'operation Err(NetFault::Unmet(report));
                            }
                            outcome
                        }
                        "tls.accept" => {
                            let server = server(arg(a, 2)?)?
                                .ok_or_else(|| invalid("server checked before accept"))?;
                            let s = self.net_streams.get_mut(&id).unwrap();
                            let outcome = if matches!(s.steps.front(), Some(NetStep::Stall)) {
                                Err(if s.read_timeout.is_some() {
                                    NetFault::TimedOut
                                } else {
                                    NetFault::Blocked
                                })
                            } else {
                                match hirsel_run_config::tls::scripted_accept(&s.peer, &server) {
                                    Ok(chain) => {
                                        s.tls = true;
                                        s.chain = chain;
                                        Ok(arg(a, 1)?.clone())
                                    }
                                    Err(failure) => Err(NetFault::Tls(failure)),
                                }
                            };
                            if outcome.as_ref().is_err_and(|e| *e != NetFault::Blocked)
                                && let Some(report) = self.close_net(id)
                            {
                                break 'operation Err(NetFault::Unmet(report));
                            }
                            outcome
                        }
                        "tls.close" => {
                            let outcome = s.check().map(|()| Value::Unit);
                            match self.close_net(id) {
                                Some(report) => Err(NetFault::Unmet(report)),
                                None => outcome,
                            }
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
                        "net.peer_addr" if hirsel_run_config::canonical_unix_path(&s.host) => {
                            Ok(addr_value(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0))
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
            Err(NetFault::Mismatch(report) | NetFault::Unmet(report)) => {
                cx.stdout.extend_from_slice(report.as_bytes());
                let failure =
                    super::dispatch::fail(call, cx, "LANG-9", "scripted net client unmet");
                if name != "net.close" {
                    self.record(
                        "Net",
                        name,
                        encode_arguments(a),
                        b"failure:LANG-9".to_vec(),
                        cx,
                    )?;
                }
                return Ok(OperationResult::Failed(failure));
            }
            Err(NetFault::Blocked) => {
                let anchor = cx.anchor().cloned().unwrap_or(crate::observation::Anchor {
                    definition: call.path.clone(),
                    body: hirsel_tir::BodyId(0),
                    cleanup: None,
                    site: call.site,
                });
                let target = if matches!(
                    name,
                    "net.connect" | "net.connect_addr" | "net.unix_connect"
                ) {
                    net
                } else if name.starts_with("udp.") {
                    self.id(arg(a, 1)?, "UdpSocket")?
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
        // Records name certificate chains and trust sets by digest; key material is never
        // recorded.
        let chain_digest = |identity: &Value| -> Result<Value, Error> {
            let chain = bytes(fields(identity, "TlsIdentity", 2)?[0])?;
            Ok(Value::Bytes(Sha256::digest(chain).to_vec()))
        };
        let input = if matches!(name, "tls.connect" | "tls.connect_as") {
            let (tag, payload) = variant_fields(arg(a, 3)?)?;
            let digest = match (tag, payload) {
                (0, []) => variant(0, vec![Value::String("webpki-roots@1.0.9".into())]),
                (1, [Some(pem)]) => {
                    variant(1, vec![Value::Bytes(Sha256::digest(bytes(pem)?).to_vec())])
                }
                _ => return Err(invalid("TlsRoots representation")),
            };
            let mut input = vec![a[0].clone(), a[1].clone(), a[2].clone(), digest];
            if name == "tls.connect_as" {
                input.push(chain_digest(arg(a, 4)?)?);
            }
            encode_arguments(&input)
        } else if name == "tls.accept" {
            let f = fields(arg(a, 2)?, "TlsServer", 2)?;
            let identities = collection::elements(f[0], BuiltinType::List)?
                .iter()
                .map(chain_digest)
                .collect::<Result<Vec<_>, _>>()?;
            let (tag, payload) = variant_fields(f[1])?;
            let clients = match (tag, payload) {
                (0, []) => variant(0, vec![]),
                (1 | 2, [Some(pem)]) => variant(
                    tag,
                    vec![Value::Bytes(Sha256::digest(bytes(pem)?).to_vec())],
                ),
                _ => return Err(invalid("TlsClientAuth representation")),
            };
            encode_arguments(&[
                a[0].clone(),
                a[1].clone(),
                Value::Tuple(identities.into_iter().map(Some).collect()),
                clients,
            ])
        } else if name == "net.read" || name == "net.read_into" || name == "udp.recv_from" {
            let max = if name == "net.read" || name == "udp.recv_from" {
                arg(a, 3)?.clone()
            } else {
                u64_value(collection::elements(arg(a, 2)?, BuiltinType::Inline)?.len() as u64)
            };
            encode_arguments(&[a[0].clone(), a[1].clone(), max])
        } else {
            encode_arguments(a)
        };
        if matches!(name, "net.listen" | "net.unix_listen")
            && let Value::Variant { variant: 0, fields } = &value
            && let Some(Some(Value::Handle { id, .. })) = fields.first()
        {
            let address = self.net_listeners[id].local();
            recorded = Some(encode_arguments(&[
                value.clone(),
                addr_value(address.ip(), address.port()),
            ]));
        }
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
            if matches!(
                name,
                "net.close"
                    | "udp.close"
                    | "tls.connect"
                    | "tls.connect_as"
                    | "tls.accept"
                    | "tls.close"
            ) {
                consumed.push(1);
            }
        }
        Ok(result)
    }
}
/// A `TlsIdentity` record loaded by the shared argument check; `None` when invalid.
fn identity(v: &Value) -> Result<Option<hirsel_run_config::tls::Identity>, Error> {
    let f = fields(v, "TlsIdentity", 2)?;
    Ok(hirsel_run_config::tls::identity(bytes(f[0])?, bytes(f[1])?))
}
/// A `TlsServer` record loaded by the shared argument check; `None` when invalid.
fn server(v: &Value) -> Result<Option<hirsel_run_config::tls::Server>, Error> {
    use hirsel_run_config::tls::ClientAuth;
    let f = fields(v, "TlsServer", 2)?;
    let identities = collection::elements(f[0], BuiltinType::List)?;
    let pairs = identities
        .iter()
        .map(|identity| {
            let f = fields(identity, "TlsIdentity", 2)?;
            Ok((bytes(f[0])?, bytes(f[1])?))
        })
        .collect::<Result<Vec<(&[u8], &[u8])>, Error>>()?;
    let (tag, payload) = variant_fields(f[1])?;
    let clients = match (tag, payload) {
        (0, []) => ClientAuth::Anonymous,
        (1, [Some(pem)]) => ClientAuth::Request(bytes(pem)?),
        (2, [Some(pem)]) => ClientAuth::Require(bytes(pem)?),
        _ => return Err(invalid("TlsClientAuth representation")),
    };
    Ok(hirsel_run_config::tls::server(&pairs, clients))
}
fn validate_peers(peers: &[NetPeer]) -> Result<(), Error> {
    let mut seen = BTreeMap::new();
    for (index, peer) in peers.iter().enumerate() {
        if let Some(first) = seen.insert((&peer.address, peer.port), index) {
            return Err(invalid(format!(
                "duplicate address/port at entries {first} and {index}"
            )));
        }
        if hirsel_run_config::check_peer_address(peer).is_err()
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
        hirsel_run_config::check_net_listeners(&grant.listeners).map_err(invalid)?;
        if grant.rules.iter().any(|r| {
            let unix = hirsel_run_config::UNIX_OPERATIONS.contains(&r.operation.as_str());
            unix && (!hirsel_run_config::canonical_unix_path(&r.address) || r.service != "0")
                || !unix && !canonical_address(&r.address)
                || r.service.parse::<u16>().is_err()
                || r.service
                    .parse::<u16>()
                    .is_ok_and(|p| p.to_string() != r.service)
                || !unix
                    && !matches!(
                        r.operation.as_str(),
                        "connect"
                            | "connect_tls"
                            | "listen"
                            | "udp_bind"
                            | "udp_peer"
                            | "listen_tls"
                    )
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
            Ok(BTreeMap::from([(
                0,
                World::new(peers.clone(), grant.unwrap().listeners.clone()),
            )]))
        }
        None => Ok(BTreeMap::from([(0, World::new(vec![], vec![]))])),
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
            .waiting_call(name, call, context, self.tasks.canceled());
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
        if matches!(
            wait.name.as_str(),
            "net.connect" | "net.connect_addr" | "net.unix_connect"
        ) && self.tasks.canceled().is_none()
        {
            return Ok(None);
        }
        let saved = (!context.configuration.inputs.is_empty()).then(|| self.simulation.clone());
        let result = self.simulation.waiting_call(
            &wait.name,
            &OperationCall {
                runtime: &Default::default(),
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
