//! Network capability data, the provider seam and grant policy.
//!
//! Policy runs in `io::State` before any provider call: a denied or invalid request never
//! reaches a provider, so it issues no syscall, DNS query or TLS byte. Providers implement
//! only host (or scripted, or recorded) effects; they never decide authority.
use crate::io::AdapterError;
pub mod tape;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum NetOperation {
    Connect,
    ConnectTls,
    Listen,
}
impl NetOperation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::ConnectTls => "connect_tls",
            Self::Listen => "listen",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "connect" => Self::Connect,
            "connect_tls" => Self::ConnectTls,
            "listen" => Self::Listen,
            _ => return None,
        })
    }
}

/// One exact grant triple (CAP-1): canonical address, exact port, operation.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct NetRule {
    pub address: String,
    pub port: u16,
    pub operation: NetOperation,
}
impl NetRule {
    /// The validated RunConfig rule set (`io.net.rules`), already canonical and unique.
    pub(crate) fn from_config(grant: &hirsel_run_config::NetGrant) -> BTreeSet<Self> {
        grant
            .rules
            .iter()
            .map(|rule| Self {
                address: rule.address.clone(),
                port: rule.service.parse().expect("validated net service"),
                operation: NetOperation::parse(&rule.operation).expect("validated net operation"),
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetTimeout {
    Unbounded,
    Nanos(u64),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownHow {
    Read,
    Write,
    Both,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeoutKind {
    Read,
    Write,
}

/// `core@1` SocketAddr: Rust's address without `flowinfo`/`scope_id`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct SocketAddr {
    pub ip: std::net::IpAddr,
    pub port: u16,
}
impl SocketAddr {
    pub fn std(self) -> std::net::SocketAddr {
        std::net::SocketAddr::new(self.ip, self.port)
    }
    pub fn of(address: std::net::SocketAddr) -> Self {
        Self {
            ip: address.ip(),
            port: address.port(),
        }
    }
}

/// `TlsFailure`, in its docs/spec/core-library.md variant order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TlsFailure {
    UnknownIssuer,
    Expired,
    NotYetValid,
    NameMismatch,
    Revoked,
    BadCertificate,
    AlertReceived(u8),
    HandshakeFailure,
    PeerMisbehaved,
    Truncated,
}
/// Scripted `Fails(f)` peers carry the same closed set in RunConfig data.
impl From<hirsel_run_config::TlsFailure> for TlsFailure {
    fn from(failure: hirsel_run_config::TlsFailure) -> Self {
        use hirsel_run_config::TlsFailure as F;
        match failure {
            F::UnknownIssuer => Self::UnknownIssuer,
            F::Expired => Self::Expired,
            F::NotYetValid => Self::NotYetValid,
            F::NameMismatch => Self::NameMismatch,
            F::Revoked => Self::Revoked,
            F::BadCertificate => Self::BadCertificate,
            F::AlertReceived(alert) => Self::AlertReceived(alert),
            F::HandshakeFailure => Self::HandshakeFailure,
            F::PeerMisbehaved => Self::PeerMisbehaved,
            F::Truncated => Self::Truncated,
        }
    }
}
impl From<TlsFailure> for hirsel_run_config::TlsFailure {
    fn from(failure: TlsFailure) -> Self {
        use TlsFailure as F;
        match failure {
            F::UnknownIssuer => Self::UnknownIssuer,
            F::Expired => Self::Expired,
            F::NotYetValid => Self::NotYetValid,
            F::NameMismatch => Self::NameMismatch,
            F::Revoked => Self::Revoked,
            F::BadCertificate => Self::BadCertificate,
            F::AlertReceived(alert) => Self::AlertReceived(alert),
            F::HandshakeFailure => Self::HandshakeFailure,
            F::PeerMisbehaved => Self::PeerMisbehaved,
            F::Truncated => Self::Truncated,
        }
    }
}

/// `NetError`, in its docs/spec/core-library.md variant order. Fault codes are capability inputs, not errno.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetError {
    NotGranted,
    Canceled(u32),
    InvalidInput,
    UnknownHost,
    ResolverFailure,
    ConnectionRefused,
    ConnectionReset,
    ConnectionAborted,
    NotConnected,
    AddrInUse,
    AddrNotAvailable,
    HostUnreachable,
    NetworkUnreachable,
    NetworkDown,
    BrokenPipe,
    TimedOut,
    Tls(TlsFailure),
    Fault(u32),
}

/// The trust configuration a `TlsConnect` request names: the compiled
/// root-set version for `WebPki()`, or the SHA-256 of the PEM bundle for `Pem`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RootsDigest {
    WebPki(String),
    Pem([u8; 32]),
}

/// Which world serves a Net: the host network, or a scripted world kept in `io::State`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetWorld {
    Host,
    Script(u64),
}

/// A provider answer: the inner result is the operation's data outcome, the outer error a
/// tool outcome (replay refusal, CONC-8 quiescence), never a language value.
pub type Provided<T> = Result<Result<T, NetError>, AdapterError>;

/// Host, scripted or recorded network effects, keyed by the logical handle the policy
/// layer assigns. Only requests the policy admitted reach a provider; a provider never
/// returns `NotGranted`.
pub trait NetProvider {
    /// Material for the next Pem handshake, supplied outside the recorded request.
    /// Scripted and replay providers need only the request's digest.
    fn tls_roots(&mut self, _pem: &[u8]) {}
    fn resolve(&mut self, host: &str, port: u16) -> Provided<Vec<SocketAddr>>;
    fn connect(&mut self, id: u64, host: &str, port: u16, timeout: NetTimeout) -> Provided<()>;
    fn connect_addr(&mut self, id: u64, addr: SocketAddr, timeout: NetTimeout) -> Provided<()>;
    fn read(&mut self, stream: u64, max: u64) -> Provided<Vec<u8>>;
    fn write(&mut self, stream: u64, bytes: &[u8]) -> Provided<u64>;
    fn write_all(&mut self, stream: u64, bytes: &[u8]) -> Provided<()>;
    fn shutdown(&mut self, stream: u64, how: ShutdownHow) -> Provided<()>;
    fn set_timeout(&mut self, stream: u64, which: TimeoutKind, timeout: NetTimeout)
    -> Provided<()>;
    fn peer_addr(&mut self, stream: u64) -> Provided<SocketAddr>;
    fn tls_connect(&mut self, stream: u64, server_name: &str, roots: &RootsDigest) -> Provided<()>;
    /// Send close_notify. The policy layer closes the stream afterwards, on every outcome,
    /// as it does after a failed `tls_connect`.
    fn tls_close(&mut self, stream: u64) -> Provided<()>;
    fn listen(&mut self, id: u64, addr: SocketAddr) -> Provided<()>;
    fn accept(&mut self, listener: u64, id: u64) -> Provided<SocketAddr>;
    fn local_addr(&mut self, listener: u64) -> Provided<SocketAddr>;
    /// Drop or `net.close`: infallible, close errors are discarded (DROP-3).
    fn close(&mut self, handle: u64);
}

/// A world with no network: every admitted operation is a tool refusal. SimIo uses it
/// between events; launches without a host network never reach it after policy.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NoNet;
impl NetProvider for NoNet {
    fn resolve(&mut self, _: &str, _: u16) -> Provided<Vec<SocketAddr>> {
        Err(AdapterError::InvalidCompletion)
    }
    fn connect(&mut self, _: u64, _: &str, _: u16, _: NetTimeout) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn connect_addr(&mut self, _: u64, _: SocketAddr, _: NetTimeout) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn read(&mut self, _: u64, _: u64) -> Provided<Vec<u8>> {
        Err(AdapterError::InvalidCompletion)
    }
    fn write(&mut self, _: u64, _: &[u8]) -> Provided<u64> {
        Err(AdapterError::InvalidCompletion)
    }
    fn write_all(&mut self, _: u64, _: &[u8]) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn shutdown(&mut self, _: u64, _: ShutdownHow) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn set_timeout(&mut self, _: u64, _: TimeoutKind, _: NetTimeout) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn peer_addr(&mut self, _: u64) -> Provided<SocketAddr> {
        Err(AdapterError::InvalidCompletion)
    }
    fn tls_connect(&mut self, _: u64, _: &str, _: &RootsDigest) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn tls_close(&mut self, _: u64) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn listen(&mut self, _: u64, _: SocketAddr) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn accept(&mut self, _: u64, _: u64) -> Provided<SocketAddr> {
        Err(AdapterError::InvalidCompletion)
    }
    fn local_addr(&mut self, _: u64) -> Provided<SocketAddr> {
        Err(AdapterError::InvalidCompletion)
    }
    fn close(&mut self, _: u64) {}
}

/// Canonical host name or IP literal: the exact form RunConfig rules use, so a request
/// names a rule address byte for byte or is `InvalidInput()`.
pub fn canonical_address(address: &str) -> bool {
    hirsel_run_config::canonical_net_address(address)
}

/// The canonical literal of an address (Rust `Display`, RFC 5952 for IPv6).
pub(crate) fn ip_literal(ip: std::net::IpAddr) -> String {
    ip.to_string()
}

/// Deno `NetDescriptor::matches_allow`, restricted to exact host and port.
pub fn permits(
    rules: &BTreeSet<NetRule>,
    address: &str,
    port: u16,
    operation: Option<NetOperation>,
) -> Option<NetOperation> {
    rules
        .iter()
        .filter(|rule| rule.address == address && rule.port == port)
        .map(|rule| rule.operation)
        .filter(|op| match operation {
            None => true,
            Some(NetOperation::Listen) => *op == NetOperation::Listen,
            // Connect is the wider grant; prefer it so a stream is plaintext-capable.
            Some(_) => matches!(op, NetOperation::Connect | NetOperation::ConnectTls),
        })
        .min()
}

/// `net.narrow` coverage: equality, or `(a, p, ConnectTls)` under `(a, p, Connect)`.
pub fn covers(rules: &BTreeSet<NetRule>, requested: &NetRule) -> bool {
    rules.contains(requested)
        || (requested.operation == NetOperation::ConnectTls
            && rules.contains(&NetRule {
                operation: NetOperation::Connect,
                ..requested.clone()
            }))
}

pub(crate) fn is_net_request(request: &crate::io::Request) -> bool {
    use crate::io::Request as R;
    matches!(
        request,
        R::TestNet { .. }
            | R::NetNarrow { .. }
            | R::Resolve { .. }
            | R::Connect { .. }
            | R::ConnectAddr { .. }
            | R::NetRead { .. }
            | R::NetWrite { .. }
            | R::NetWriteAll { .. }
            | R::Shutdown { .. }
            | R::SetTimeout { .. }
            | R::PeerAddr { .. }
            | R::TlsConnect { .. }
            | R::TlsClose { .. }
            | R::Listen { .. }
            | R::Accept { .. }
            | R::LocalAddr { .. }
    )
}

/// Providers never decide authority: a provider `NotGranted` is a refused input.
fn provided<T>(value: Provided<T>) -> Provided<T> {
    match value {
        Ok(Err(NetError::NotGranted)) => Err(AdapterError::InvalidCompletion),
        other => other,
    }
}

struct StreamGrant {
    world: NetWorld,
    address: String,
    operation: NetOperation,
    tls: bool,
}

impl crate::io::State {
    pub(crate) fn remember_tls_roots(&mut self, stream: u64, pem: &[u8]) {
        let RootsDigest::Pem(digest) = crate::net_tls::pem_digest(pem) else {
            unreachable!()
        };
        self.pem_validity.insert(
            stream,
            (digest, hirsel_run_config::tls::pem_roots(pem).is_some()),
        );
    }

    pub(crate) fn set_net_script(
        &mut self,
        peers: Vec<hirsel_run_config::NetPeer>,
        listeners: Vec<hirsel_run_config::NetListener>,
    ) -> Result<(), AdapterError> {
        let script = crate::io_net_script::ScriptNet::new(peers, listeners)?;
        self.net_scripts.insert(0, script);
        self.net_world = NetWorld::Script(0);
        Ok(())
    }
    fn net_grant(&self, net: u64) -> Result<(BTreeSet<NetRule>, NetWorld), AdapterError> {
        match self.handles.rows.get(&net) {
            Some(crate::handles::Grant::Net { rules, world }) => Ok((rules.clone(), *world)),
            _ => Err(AdapterError::InvalidHandle(net)),
        }
    }
    /// The stream's own grant, reached only through the Net that brands it (PROV-1).
    fn stream_grant(&self, net: u64, stream: u64) -> Result<StreamGrant, AdapterError> {
        let (_, world) = self.net_grant(net)?;
        match self.handles.rows.get(&stream) {
            Some(crate::handles::Grant::Stream {
                net: owner,
                address,
                operation,
                tls,
                ..
            }) if *owner == net => Ok(StreamGrant {
                world,
                address: address.clone(),
                operation: *operation,
                tls: *tls,
            }),
            _ => Err(AdapterError::InvalidHandle(stream)),
        }
    }
    fn listener_grant(&self, net: u64, listener: u64) -> Result<NetWorld, AdapterError> {
        let (_, world) = self.net_grant(net)?;
        match self.handles.rows.get(&listener) {
            Some(crate::handles::Grant::Listener { net: owner }) if *owner == net => Ok(world),
            _ => Err(AdapterError::InvalidHandle(listener)),
        }
    }
    /// Select the provider serving a world. Scripted worlds (`test.net`, `net.script@2`)
    /// attach here.
    fn world<'a>(
        &'a mut self,
        world: NetWorld,
        host: &'a mut dyn NetProvider,
    ) -> Result<&'a mut dyn NetProvider, AdapterError> {
        match world {
            NetWorld::Host => Ok(host),
            NetWorld::Script(id) => match self.net_scripts.get_mut(&id) {
                Some(script) => Ok(script),
                None => Err(AdapterError::InvalidHandle(id)),
            },
        }
    }
    /// Before closing a scripted stream or listener: a client left with an unmet
    /// Expect fails LANG-9 after its Test report.
    fn unmet(&mut self, world: NetWorld, handle: u64) -> Option<crate::io::Completion> {
        let NetWorld::Script(id) = world else {
            return None;
        };
        let report = self.net_scripts.get_mut(&id)?.unmet(handle)?;
        self.stdout.extend_from_slice(report.as_bytes());
        Some(crate::io::Completion::Checked(false))
    }
    fn mark_tls(&mut self, stream: u64) {
        if let Some(crate::handles::Grant::Stream { tls, .. }) = self.handles.rows.get_mut(&stream)
        {
            *tls = true;
        }
    }
    /// Program errors only: never consult or advance a provider (CONC-3).
    pub(crate) fn net_error(
        &self,
        request: &crate::io::Request,
    ) -> Result<Option<NetError>, AdapterError> {
        use crate::io::Request as R;
        let zero = |t: &NetTimeout| *t == NetTimeout::Nanos(0);
        Ok(match request {
            R::Resolve { net, host, port }
            | R::Connect {
                net, host, port, ..
            } => {
                let (rules, _) = self.net_grant(*net)?;
                if !canonical_address(host)
                    || matches!(request, R::Connect { timeout, .. } if zero(timeout))
                {
                    Some(NetError::InvalidInput)
                } else if permits(
                    &rules,
                    host,
                    *port,
                    if matches!(request, R::Connect { .. }) {
                        Some(NetOperation::Connect)
                    } else {
                        None
                    },
                )
                .is_none()
                {
                    Some(NetError::NotGranted)
                } else {
                    None
                }
            }
            R::ConnectAddr { net, addr, timeout } => {
                let (rules, _) = self.net_grant(*net)?;
                if zero(timeout) {
                    Some(NetError::InvalidInput)
                } else if permits(
                    &rules,
                    &ip_literal(addr.ip),
                    addr.port,
                    Some(NetOperation::Connect),
                )
                .is_none()
                {
                    Some(NetError::NotGranted)
                } else {
                    None
                }
            }
            R::NetRead { net, stream, .. }
            | R::NetWrite { net, stream, .. }
            | R::NetWriteAll { net, stream, .. }
            | R::Shutdown { net, stream, .. }
            | R::SetTimeout { net, stream, .. }
            | R::PeerAddr { net, stream }
            | R::TlsConnect { net, stream, .. }
            | R::TlsClose { net, stream } => {
                let grant = self.stream_grant(*net, *stream)?;
                let invalid_tls = match request {
                    R::TlsConnect {
                        server_name, roots, ..
                    } => {
                        hirsel_run_config::tls::server_name(server_name).is_none()
                            || match roots {
                                RootsDigest::WebPki(version) => {
                                    version != crate::net_tls::WEBPKI_VERSION
                                }
                                RootsDigest::Pem(digest) => {
                                    self.pem_validity.get(stream) != Some(&(*digest, true))
                                }
                            }
                    }
                    _ => false,
                };
                if invalid_tls
                    || matches!(request, R::NetRead { max: 0, .. })
                    || matches!(request, R::SetTimeout { timeout, .. } if zero(timeout))
                    || matches!(request, R::TlsConnect { .. } if grant.tls)
                {
                    Some(NetError::InvalidInput)
                } else if grant.operation == NetOperation::ConnectTls
                    && (matches!(request, R::TlsConnect { server_name, .. } if *server_name != grant.address)
                        || !grant.tls
                            && matches!(
                                request,
                                R::NetRead { .. }
                                    | R::NetWrite { .. }
                                    | R::NetWriteAll { .. }
                                    | R::Shutdown { .. }
                            ))
                {
                    Some(NetError::NotGranted)
                } else {
                    None
                }
            }
            R::Accept { net, listener } | R::LocalAddr { net, listener } => {
                self.listener_grant(*net, *listener)?;
                None
            }
            _ => None,
        })
    }
    /// Check every grant before any provider effect (CAP-1), then ask the
    /// provider of the Net's world. Input validation precedes the grant check (docs/spec/core-library.md).
    pub(crate) fn execute_net(
        &mut self,
        host: &mut dyn NetProvider,
        request: &crate::io::Request,
    ) -> Result<crate::io::Completion, AdapterError> {
        use crate::handles::Grant;
        use crate::io::{Completion as C, Request as R};
        if let Some(error) = self.net_error(request)? {
            if let R::TlsConnect { stream, .. } = request {
                let closed = self.execute_net(host, &R::Close { handle: *stream })?;
                if matches!(closed, C::Checked(false)) {
                    return Ok(closed);
                }
            }
            return Ok(crate::io_checks::net_completion(request, error));
        }
        match request {
            R::TestNet {
                test,
                peers,
                listeners,
            } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                let script =
                    crate::io_net_script::ScriptNet::new(peers.clone(), listeners.clone())?;
                let rules = peers
                    .iter()
                    .map(|peer| NetRule {
                        address: peer.address.clone(),
                        port: peer.port,
                        operation: match &peer.tls {
                            hirsel_run_config::PeerTls::Plain => NetOperation::Connect,
                            hirsel_run_config::PeerTls::Certificate(_)
                            | hirsel_run_config::PeerTls::Fails(_) => NetOperation::ConnectTls,
                        },
                    })
                    .chain(listeners.iter().map(|listener| NetRule {
                        address: listener.address.clone(),
                        port: listener.port,
                        operation: NetOperation::Listen,
                    }))
                    .collect();
                let id = self.handles.next();
                self.handles.insert(Grant::Net {
                    rules,
                    world: NetWorld::Script(id),
                });
                self.net_scripts.insert(id, script);
                Ok(C::Handle(Ok(id)))
            }
            R::NetNarrow { net, rules } => {
                let (parent, world) = self.net_grant(*net)?;
                let rules: BTreeSet<NetRule> = rules.iter().cloned().collect();
                if !rules.iter().all(|rule| covers(&parent, rule)) {
                    return Ok(C::NetHandle(Err(NetError::NotGranted)));
                }
                Ok(C::NetHandle(Ok(self
                    .handles
                    .insert(Grant::Net { rules, world }))))
            }
            R::Resolve {
                net,
                host: name,
                port,
            } => {
                let (_, world) = self.net_grant(*net)?;
                let value = provided(self.world(world, host)?.resolve(name, *port))?;
                if let Ok(addresses) = &value
                    && addresses.iter().any(|a| a.port != *port)
                {
                    return Err(AdapterError::InvalidCompletion);
                }
                Ok(C::Addresses(value))
            }
            R::Connect {
                net,
                host: name,
                port,
                timeout,
            } => {
                let (rules, world) = self.net_grant(*net)?;
                let operation = permits(&rules, name, *port, Some(NetOperation::Connect)).unwrap();
                let id = self.handles.next();
                let value = provided(self.world(world, host)?.connect(id, name, *port, *timeout))?;
                Ok(C::NetHandle(value.map(|()| {
                    self.handles.insert(Grant::Stream {
                        net: *net,
                        address: name.clone(),
                        port: *port,
                        operation,
                        tls: false,
                    })
                })))
            }
            R::ConnectAddr { net, addr, timeout } => {
                let (rules, world) = self.net_grant(*net)?;
                let literal = ip_literal(addr.ip);
                let operation =
                    permits(&rules, &literal, addr.port, Some(NetOperation::Connect)).unwrap();
                let id = self.handles.next();
                let value = provided(self.world(world, host)?.connect_addr(id, *addr, *timeout))?;
                Ok(C::NetHandle(value.map(|()| {
                    self.handles.insert(Grant::Stream {
                        net: *net,
                        address: literal,
                        port: addr.port,
                        operation,
                        tls: false,
                    })
                })))
            }
            R::NetRead { net, stream, .. }
            | R::NetWrite { net, stream, .. }
            | R::NetWriteAll { net, stream, .. }
            | R::Shutdown { net, stream, .. }
            | R::SetTimeout { net, stream, .. }
            | R::PeerAddr { net, stream } => {
                let grant = self.stream_grant(*net, *stream)?;
                let provider = self.world(grant.world, host)?;
                let completion = match request {
                    R::NetRead { max, .. } => {
                        C::NetBytes(provided(provider.read(*stream, *max))?.and_then(|bytes| {
                            if bytes.len() as u64 > *max {
                                Err(NetError::InvalidInput)
                            } else {
                                Ok(bytes)
                            }
                        }))
                    }
                    R::NetWrite { bytes, .. } => {
                        C::Count(provided(provider.write(*stream, bytes))?)
                    }
                    R::NetWriteAll { bytes, .. } => {
                        C::NetUnit(provided(provider.write_all(*stream, bytes))?)
                    }
                    R::Shutdown { how, .. } => {
                        C::NetUnit(provided(provider.shutdown(*stream, *how))?)
                    }
                    R::SetTimeout { which, timeout, .. } => {
                        C::NetUnit(provided(provider.set_timeout(*stream, *which, *timeout))?)
                    }
                    R::PeerAddr { .. } => C::Address(provided(provider.peer_addr(*stream))?),
                    _ => unreachable!(),
                };
                // A write that contradicts a scripted client's Expect fails LANG-9 after
                // its Test report (docs/spec/core-library.md, scripted peers).
                if let NetWorld::Script(world) = grant.world
                    && let Some(report) = self
                        .net_scripts
                        .get_mut(&world)
                        .and_then(|script| script.take_mismatch(*stream))
                {
                    self.stdout.extend_from_slice(report.as_bytes());
                    return Ok(C::Checked(false));
                }
                Ok(completion)
            }
            R::TlsConnect {
                net,
                stream,
                server_name,
                roots,
            } => {
                let grant = self.stream_grant(*net, *stream)?;
                // The stream is taken: it is closed on every failure.
                let value = provided(self.world(grant.world, host)?.tls_connect(
                    *stream,
                    server_name,
                    roots,
                ))?;
                self.pem_validity.remove(stream);
                if value.is_ok() {
                    self.mark_tls(*stream);
                } else {
                    let closed = self.execute_net(host, &R::Close { handle: *stream })?;
                    if matches!(closed, C::Checked(false)) {
                        return Ok(closed);
                    }
                }
                Ok(C::NetUnit(value))
            }
            R::TlsClose { net, stream } => {
                let grant = self.stream_grant(*net, *stream)?;
                let value = provided(self.world(grant.world, host)?.tls_close(*stream))?;
                let closed = self.execute_net(host, &R::Close { handle: *stream })?;
                Ok(if matches!(closed, C::Checked(false)) {
                    closed
                } else {
                    C::NetUnit(value)
                })
            }
            R::Listen { net, addr } => {
                let (rules, world) = self.net_grant(*net)?;
                let literal = ip_literal(addr.ip);
                if permits(&rules, &literal, addr.port, Some(NetOperation::Listen)).is_none() {
                    return Ok(C::Listened(Err(NetError::NotGranted)));
                }
                let id = self.handles.next();
                if let Err(error) = provided(self.world(world, host)?.listen(id, *addr))? {
                    return Ok(C::Listened(Err(error)));
                }
                let local = provided(self.world(world, host)?.local_addr(id))?
                    .map_err(|_| AdapterError::InvalidCompletion)?;
                if local.ip != addr.ip
                    || local.port == 0
                    || (addr.port != 0 && local.port != addr.port)
                {
                    return Err(AdapterError::InvalidCompletion);
                }
                let id = self.handles.insert(Grant::Listener { net: *net });
                Ok(C::Listened(Ok((id, local))))
            }
            R::Accept { net, listener } => {
                let world = self.listener_grant(*net, *listener)?;
                let id = self.handles.next();
                let value = provided(self.world(world, host)?.accept(*listener, id))?;
                // Accepted peers are not checked: the listen rule is the authority.
                Ok(C::Accepted(value.map(|peer| {
                    let id = self.handles.insert(Grant::Stream {
                        net: *net,
                        address: ip_literal(peer.ip),
                        port: peer.port,
                        operation: NetOperation::Connect,
                        tls: false,
                    });
                    (id, peer)
                })))
            }
            R::LocalAddr { net, listener } => {
                let world = self.listener_grant(*net, *listener)?;
                Ok(C::Address(provided(
                    self.world(world, host)?.local_addr(*listener),
                )?))
            }
            R::Close { handle } => {
                self.pem_validity.remove(handle);
                let world = match self.handles.rows.remove(handle) {
                    Some(Grant::Stream { net, .. } | Grant::Listener { net }) => {
                        self.net_grant(net)?.1
                    }
                    _ => return Err(AdapterError::InvalidHandle(*handle)),
                };
                let unmet = self.unmet(world, *handle);
                self.world(world, host)?.close(*handle);
                Ok(unmet.unwrap_or(C::Unit(Ok(()))))
            }
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
}
