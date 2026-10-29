//! Network capability data, the provider seam and grant policy.
//!
//! Policy runs in `io::State` before any provider call: a denied or invalid request never
//! reaches a provider, so it issues no syscall, DNS query or TLS byte. Providers implement
//! only host (or scripted, or recorded) effects; they never decide authority.
use crate::io::AdapterError;
use crate::net_tls::TlsEngine;
pub mod tape;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum NetOperation {
    Connect,
    ConnectTls,
    Listen,
    UdpBind,
    UdpPeer,
    ListenTls,
    UnixConnect,
    UnixListen,
    UnixListenRemoveStale,
}
impl NetOperation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::ConnectTls => "connect_tls",
            Self::Listen => "listen",
            Self::UdpBind => "udp_bind",
            Self::UdpPeer => "udp_peer",
            Self::ListenTls => "listen_tls",
            Self::UnixConnect => "unix_connect",
            Self::UnixListen => "unix_listen",
            Self::UnixListenRemoveStale => "unix_listen_remove_stale",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "connect" => Self::Connect,
            "connect_tls" => Self::ConnectTls,
            "listen" => Self::Listen,
            "udp_bind" => Self::UdpBind,
            "udp_peer" => Self::UdpPeer,
            "listen_tls" => Self::ListenTls,
            "unix_connect" => Self::UnixConnect,
            "unix_listen" => Self::UnixListen,
            "unix_listen_remove_stale" => Self::UnixListenRemoveStale,
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
    pub(crate) fn from_config(grant: &hirsel_run_schema::NetGrant) -> BTreeSet<Self> {
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
    /// The address of an unnamed Unix peer, and of every Unix stream or listener:
    /// `0.0.0.0` with port 0 (docs/spec/core-library.md, Unix domain sockets).
    pub const UNNAMED: Self = Self {
        ip: std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        port: 0,
    };
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
    CertificateRequired,
}
/// Scripted `Fails(f)` peers carry the same closed set in RunConfig data.
impl From<hirsel_run_schema::TlsFailure> for TlsFailure {
    fn from(failure: hirsel_run_schema::TlsFailure) -> Self {
        use hirsel_run_schema::TlsFailure as F;
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
            F::CertificateRequired => Self::CertificateRequired,
        }
    }
}
impl From<TlsFailure> for hirsel_run_schema::TlsFailure {
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
            F::CertificateRequired => Self::CertificateRequired,
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

/// The client identity a `tls.connect_as` request names: the SHA-256 of the certificate-chain
/// PEM, which the request records, and the key loaded from material supplied outside the
/// tape, absent when the identity is invalid. Key bytes never reach the tape, a digest or a debug print.
#[derive(Clone)]
pub struct TlsIdentity {
    pub chain: [u8; 32],
    pub key: Option<std::sync::Arc<hirsel_run_schema::tls::Identity>>,
}
impl TlsIdentity {
    pub fn load(tls: &dyn TlsEngine, chain: &[u8], key: &[u8]) -> Self {
        Self {
            chain: crate::net_tls::chain_digest(chain),
            key: tls.identity(chain, key),
        }
    }
}
/// A valid key is determined by its chain, so the digest and validity are the whole identity.
impl PartialEq for TlsIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.chain == other.chain && self.key.is_some() == other.key.is_some()
    }
}
impl Eq for TlsIdentity {}
impl std::fmt::Debug for TlsIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsIdentity")
            .field("chain", &self.chain)
            .field("valid", &self.key.is_some())
            .finish()
    }
}

pub use hirsel_run_schema::tls::ClientAuth;

/// What a `TlsAccept` request records of its `TlsServer`: the SHA-256 of each identity's
/// certificate-chain PEM, in order, of the client trust set's PEM, and its ALPN list. Keys
/// never appear.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TlsServerDigest {
    pub identities: Vec<[u8; 32]>,
    pub clients: ClientAuth<[u8; 32]>,
    pub alpn: Vec<String>,
}
impl TlsServerDigest {
    pub fn of(identities: &[(&[u8], &[u8])], clients: ClientAuth<&[u8]>, alpn: &[String]) -> Self {
        Self {
            identities: identities
                .iter()
                .map(|(chain, _)| crate::net_tls::chain_digest(chain))
                .collect(),
            clients: clients.map(crate::net_tls::chain_digest),
            alpn: alpn.to_vec(),
        }
    }
}

/// A `TlsServer` supplied outside the tape: its digest, and its material when the whole
/// server is valid (a nonempty list of valid identities, a valid nonempty trust set and a
/// valid ALPN list).
#[derive(Clone, Debug)]
pub struct TlsServer {
    pub digest: TlsServerDigest,
    pub loaded: Option<hirsel_run_schema::tls::Server>,
}
/// Valid material is determined by its digests, so they and validity are the whole server.
impl PartialEq for TlsServer {
    fn eq(&self, other: &Self) -> bool {
        self.digest == other.digest && self.loaded.is_some() == other.loaded.is_some()
    }
}
impl Eq for TlsServer {}
impl TlsServer {
    pub fn load(
        tls: &dyn TlsEngine,
        identities: &[(&[u8], &[u8])],
        clients: ClientAuth<&[u8]>,
        alpn: &[String],
    ) -> Self {
        let digest = TlsServerDigest::of(identities, clients, alpn);
        let loaded = tls.server(identities, clients, alpn);
        Self { digest, loaded }
    }
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
    fn udp_bind(&mut self, id: u64, addr: SocketAddr) -> Provided<()>;
    fn udp_send(&mut self, socket: u64, bytes: &[u8], addr: SocketAddr) -> Provided<u64>;
    /// Receive one entire datagram, including an empty one. Policy truncates after checking its peer.
    fn udp_recv(&mut self, socket: u64) -> Provided<(Vec<u8>, SocketAddr)>;
    fn resolve(&mut self, host: &str, port: u16) -> Provided<Vec<SocketAddr>>;
    fn connect(&mut self, id: u64, host: &str, port: u16, timeout: NetTimeout) -> Provided<()>;
    fn connect_addr(&mut self, id: u64, addr: SocketAddr, timeout: NetTimeout) -> Provided<()>;
    /// A sole native root can finish on one socket without readiness bookkeeping.
    /// Decline for shared drivers, TLS or any external wake/stop source.
    fn net_io_inline(
        &mut self,
        _stream: u64,
        _buffer: &mut crate::io::NetBuffer<'_>,
    ) -> Option<Provided<u64>> {
        None
    }
    fn read(&mut self, stream: u64, max: u64) -> Provided<Vec<u8>>;
    /// Fill spare capacity, initializing only the returned prefix. Like Rust's
    /// BorrowedBuf, the unwritten suffix need not be initialized or zeroed.
    fn read_uninit(
        &mut self,
        stream: u64,
        buffer: &mut [std::mem::MaybeUninit<u8>],
    ) -> Provided<usize> {
        self.read(stream, buffer.len() as u64).map(|value| {
            value.map(|bytes| {
                assert!(bytes.len() <= buffer.len());
                for (slot, byte) in buffer.iter_mut().zip(&bytes) {
                    slot.write(*byte);
                }
                bytes.len()
            })
        })
    }
    fn write(&mut self, stream: u64, bytes: &[u8]) -> Provided<u64>;
    fn write_all(&mut self, stream: u64, bytes: &[u8]) -> Provided<()>;
    fn shutdown(&mut self, stream: u64, how: ShutdownHow) -> Provided<()>;
    fn set_timeout(&mut self, stream: u64, which: TimeoutKind, timeout: NetTimeout)
    -> Provided<()>;
    fn peer_addr(&mut self, stream: u64) -> Provided<SocketAddr>;
    /// A client handshake through `tls`, presenting `identity` when the server asks for a
    /// certificate and offering the ALPN protocols `alpn`. Success is the protocol the server
    /// selected.
    fn tls_connect(
        &mut self,
        tls: &dyn TlsEngine,
        stream: u64,
        server_name: &str,
        roots: &RootsDigest,
        identity: Option<&std::sync::Arc<hirsel_run_schema::tls::Identity>>,
        alpn: &[String],
    ) -> Provided<Option<String>>;
    /// A server handshake with a valid server: the identity selected by the client's SNI,
    /// its client-certificate policy and its ALPN list. Success is the protocol selected.
    /// The policy layer closes the stream after a failure, as for `tls_connect`.
    fn tls_accept(
        &mut self,
        tls: &dyn TlsEngine,
        stream: u64,
        server: &hirsel_run_schema::tls::Server,
    ) -> Provided<Option<String>>;
    /// The DER chain (leaf first, concatenated) a `tls_accept` stream's client presented and
    /// the handshake verified; `None` for any other stream.
    fn tls_peer(&mut self, stream: u64) -> Provided<Option<Vec<u8>>>;
    /// Send close_notify. The policy layer closes the stream afterwards, on every outcome,
    /// as it does after a failed `tls_connect`.
    fn tls_close(&mut self, stream: u64) -> Provided<()>;
    fn listen(&mut self, id: u64, addr: SocketAddr) -> Provided<()>;
    /// Connect a stream to the socket at a canonical `path`.
    fn unix_connect(&mut self, id: u64, path: &str) -> Provided<()>;
    /// Bind and listen at `path`, first removing a stale socket file there when
    /// `remove_stale`. Closing the listener removes the file it created.
    fn unix_listen(&mut self, id: u64, path: &str, remove_stale: bool) -> Provided<()>;
    fn accept(&mut self, listener: u64, id: u64) -> Provided<SocketAddr>;
    fn local_addr(&mut self, listener: u64) -> Provided<SocketAddr>;
    /// Drop or `net.close`: infallible, close errors are discarded (DROP-3).
    fn close(&mut self, handle: u64);
}

/// A world with no network: every admitted operation is a tool refusal. SimIo uses it
/// between events, and RealIo before a net entry handed it the host provider; launches
/// without a host network never reach it after policy.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NoNet;
impl NetProvider for NoNet {
    fn udp_bind(&mut self, _: u64, _: SocketAddr) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn udp_send(&mut self, _: u64, _: &[u8], _: SocketAddr) -> Provided<u64> {
        Err(AdapterError::InvalidCompletion)
    }
    fn udp_recv(&mut self, _: u64) -> Provided<(Vec<u8>, SocketAddr)> {
        Err(AdapterError::InvalidCompletion)
    }
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
    fn tls_connect(
        &mut self,
        _: &dyn TlsEngine,
        _: u64,
        _: &str,
        _: &RootsDigest,
        _: Option<&std::sync::Arc<hirsel_run_schema::tls::Identity>>,
        _: &[String],
    ) -> Provided<Option<String>> {
        Err(AdapterError::InvalidCompletion)
    }
    fn tls_accept(
        &mut self,
        _: &dyn TlsEngine,
        _: u64,
        _: &hirsel_run_schema::tls::Server,
    ) -> Provided<Option<String>> {
        Err(AdapterError::InvalidCompletion)
    }
    fn tls_peer(&mut self, _: u64) -> Provided<Option<Vec<u8>>> {
        Err(AdapterError::InvalidCompletion)
    }
    fn tls_close(&mut self, _: u64) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn listen(&mut self, _: u64, _: SocketAddr) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn unix_connect(&mut self, _: u64, _: &str) -> Provided<()> {
        Err(AdapterError::InvalidCompletion)
    }
    fn unix_listen(&mut self, _: u64, _: &str, _: bool) -> Provided<()> {
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
    hirsel_run_schema::canonical_net_address(address)
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
            Some(
                NetOperation::UdpBind
                | NetOperation::UdpPeer
                | NetOperation::UnixConnect
                | NetOperation::UnixListenRemoveStale,
            ) => Some(*op) == operation,
            // Listen and Connect are the wider grants; `min` prefers them so a stream is
            // plaintext-capable.
            Some(NetOperation::Listen) => {
                matches!(op, NetOperation::Listen | NetOperation::ListenTls)
            }
            // Either Unix listen rule; prefer the one that may remove a stale socket.
            Some(NetOperation::UnixListen) => matches!(
                op,
                NetOperation::UnixListen | NetOperation::UnixListenRemoveStale
            ),
            Some(_) => matches!(op, NetOperation::Connect | NetOperation::ConnectTls),
        })
        .reduce(|a, b| match (a, b) {
            (NetOperation::UnixListen, NetOperation::UnixListenRemoveStale) => b,
            _ => a.min(b),
        })
}

/// `net.narrow` coverage: a rule of the same address and port whose operation is the
/// requested one or its wider one (`hirsel_run_schema::net_operation_within`).
pub fn covers(rules: &BTreeSet<NetRule>, requested: &NetRule) -> bool {
    rules.iter().any(|rule| {
        rule.address == requested.address
            && rule.port == requested.port
            && hirsel_run_schema::net_operation_within(
                requested.operation.name(),
                rule.operation.name(),
            )
    })
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
            | R::NetSplit { .. }
            | R::NetJoin { .. }
            | R::TlsConnect { .. }
            | R::TlsAccept { .. }
            | R::TlsPeer { .. }
            | R::TlsProtocol { .. }
            | R::TlsClose { .. }
            | R::Listen { .. }
            | R::Accept { .. }
            | R::LocalAddr { .. }
            | R::UdpBind { .. }
            | R::UdpSend { .. }
            | R::UdpRecv { .. }
            | R::UnixConnect { .. }
            | R::UnixListen { .. }
    )
}

/// Providers never decide authority: a provider `NotGranted` is a refused input.
fn provided<T>(value: Provided<T>) -> Provided<T> {
    match value {
        Ok(Err(NetError::NotGranted)) => Err(AdapterError::InvalidCompletion),
        other => other,
    }
}

struct StreamGrant<'a> {
    /// The connection providers know: the stream itself, or a half's stream.
    id: u64,
    world: NetWorld,
    address: &'a str,
    operation: NetOperation,
    tls: bool,
}

impl crate::io::State {
    /// The TLS engine the program's TLS entries handed the adapter (`Adapter::tls`).
    pub(crate) fn tls(&self) -> &'static dyn TlsEngine {
        self.tls
            .0
            .unwrap_or_else(|| crate::fault("a TLS request before the TLS engine"))
    }

    pub(crate) fn remember_tls_identity(&mut self, stream: u64, chain: &[u8], key: &[u8]) {
        self.tls_identities
            .insert(stream, TlsIdentity::load(self.tls(), chain, key));
    }

    pub(crate) fn remember_tls_server(
        &mut self,
        stream: u64,
        identities: &[(&[u8], &[u8])],
        clients: ClientAuth<&[u8]>,
        alpn: &[String],
    ) {
        self.tls_servers.insert(
            stream,
            TlsServer::load(self.tls(), identities, clients, alpn),
        );
    }

    pub(crate) fn remember_tls_roots(&mut self, stream: u64, pem: &[u8]) {
        let RootsDigest::Pem(digest) = crate::net_tls::pem_digest(pem) else {
            unreachable!()
        };
        self.pem_validity
            .insert(stream, (digest, self.tls().valid_roots(pem)));
    }

    pub(crate) fn set_net_script(
        &mut self,
        peers: Vec<hirsel_run_schema::NetPeer>,
        listeners: Vec<hirsel_run_schema::NetListener>,
    ) -> Result<(), AdapterError> {
        let script = (crate::runtime_roots::script()
            .ok_or_else(|| {
                AdapterError::Unsupported("scripted network provider is not linked".into())
            })?
            .new)(peers, listeners)?;
        self.net_scripts.insert(0, script);
        self.net_world = NetWorld::Script(0);
        Ok(())
    }
    fn net_grant(&self, net: u64) -> Result<(&BTreeSet<NetRule>, NetWorld), AdapterError> {
        match self.handles.rows.get(&net) {
            Some(crate::handles::Grant::Net { rules, world }) => Ok((rules, *world)),
            _ => Err(AdapterError::InvalidHandle(net)),
        }
    }
    /// The stream's own grant, reached only through the Net that brands it (PROV-1). A
    /// half reaches its connection's grant.
    fn stream_grant(&self, net: u64, stream: u64) -> Result<StreamGrant<'_>, AdapterError> {
        let (_, world) = self.net_grant(net)?;
        let (id, grant) = match self.handles.rows.get(&stream) {
            Some(crate::handles::Grant::Half {
                net: owner,
                stream: connection,
                ..
            }) if *owner == net => (
                *connection,
                self.split_streams.get(connection).map(|(grant, _)| grant),
            ),
            grant => (stream, grant),
        };
        match grant {
            Some(crate::handles::Grant::Stream {
                net: owner,
                address,
                operation,
                tls,
                ..
            }) if *owner == net => Ok(StreamGrant {
                id,
                world,
                address,
                operation: *operation,
                tls: *tls,
            }),
            _ => Err(AdapterError::InvalidHandle(stream)),
        }
    }
    /// Whether a network request is served by a scripted world, whose waits no host event
    /// readies.
    pub(crate) fn scripted(&self, request: &crate::io::Request) -> bool {
        use crate::io::Request as R;
        let net = match request {
            R::NetNarrow { net, .. }
            | R::Resolve { net, .. }
            | R::Connect { net, .. }
            | R::ConnectAddr { net, .. }
            | R::UdpBind { net, .. }
            | R::UdpSend { net, .. }
            | R::UdpRecv { net, .. }
            | R::NetRead { net, .. }
            | R::NetWrite { net, .. }
            | R::NetWriteAll { net, .. }
            | R::Shutdown { net, .. }
            | R::SetTimeout { net, .. }
            | R::PeerAddr { net, .. }
            | R::NetSplit { net, .. }
            | R::NetJoin { net, .. }
            | R::TlsConnect { net, .. }
            | R::TlsAccept { net, .. }
            | R::TlsPeer { net, .. }
            | R::TlsProtocol { net, .. }
            | R::TlsClose { net, .. }
            | R::Listen { net, .. }
            | R::Accept { net, .. }
            | R::LocalAddr { net, .. }
            | R::UnixConnect { net, .. }
            | R::UnixListen { net, .. } => *net,
            _ => return false,
        };
        self.net_grant(net)
            .is_ok_and(|(_, world)| matches!(world, NetWorld::Script(_)))
    }
    /// A half serves only its own direction's operations; a whole stream serves them all.
    fn half_serves(&self, request: &crate::io::Request, handle: u64) -> bool {
        use crate::io::Request as R;
        match self.handles.rows.get(&handle) {
            Some(crate::handles::Grant::Half { write: false, .. }) => matches!(
                request,
                R::NetRead { .. }
                    | R::SetTimeout {
                        which: TimeoutKind::Read,
                        ..
                    }
            ),
            Some(crate::handles::Grant::Half { write: true, .. }) => matches!(
                request,
                R::NetWrite { .. }
                    | R::NetWriteAll { .. }
                    | R::Shutdown {
                        how: ShutdownHow::Write,
                        ..
                    }
                    | R::SetTimeout {
                        which: TimeoutKind::Write,
                        ..
                    }
            ),
            _ => true,
        }
    }
    /// A half of `net`: its connection and direction.
    fn half(&self, net: u64, handle: u64) -> Result<(u64, bool), AdapterError> {
        self.net_grant(net)?;
        match self.handles.rows.get(&handle) {
            Some(crate::handles::Grant::Half {
                net: owner,
                stream,
                write,
            }) if *owner == net => Ok((*stream, *write)),
            _ => Err(AdapterError::InvalidHandle(handle)),
        }
    }
    /// The listener's world and the operation of the rule that opened it.
    fn listener_grant(
        &self,
        net: u64,
        listener: u64,
    ) -> Result<(NetWorld, NetOperation), AdapterError> {
        let (_, world) = self.net_grant(net)?;
        match self.handles.rows.get(&listener) {
            Some(crate::handles::Grant::Listener {
                net: owner,
                operation,
            }) if *owner == net => Ok((world, *operation)),
            _ => Err(AdapterError::InvalidHandle(listener)),
        }
    }
    pub(crate) fn host_stream(
        &self,
        net: u64,
        stream: u64,
        write: bool,
        empty: bool,
    ) -> Result<Option<Result<u64, NetError>>, AdapterError> {
        let grant = self.stream_grant(net, stream)?;
        if !matches!(grant.world, NetWorld::Host) {
            return Ok(None);
        }
        Ok(Some(
            match self.stream_io_error(stream, &grant, write, empty)? {
                Some(error) => Err(error),
                None => Ok(grant.id),
            },
        ))
    }
    fn stream_io_error(
        &self,
        stream: u64,
        grant: &StreamGrant<'_>,
        write: bool,
        empty: bool,
    ) -> Result<Option<NetError>, AdapterError> {
        if let Some(crate::handles::Grant::Half {
            write: direction, ..
        }) = self.handles.rows.get(&stream)
            && *direction != write
        {
            return Err(AdapterError::InvalidHandle(stream));
        }
        Ok(if !write && empty {
            Some(NetError::InvalidInput)
        } else if !grant.tls
            && matches!(
                grant.operation,
                NetOperation::ConnectTls | NetOperation::ListenTls
            )
        {
            Some(NetError::NotGranted)
        } else {
            None
        })
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
    /// A successful handshake: the stream carries plaintext and its negotiated protocol.
    fn mark_tls(&mut self, stream: u64, negotiated: Option<String>) {
        if let Some(crate::handles::Grant::Stream { tls, protocol, .. }) =
            self.handles.rows.get_mut(&stream)
        {
            *tls = true;
            *protocol = negotiated;
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
            R::UdpBind { net, addr } => {
                let (rules, _) = self.net_grant(*net)?;
                permits(
                    &rules,
                    &ip_literal(addr.ip),
                    addr.port,
                    Some(NetOperation::UdpBind),
                )
                .is_none()
                .then_some(NetError::NotGranted)
            }
            R::UdpSend { net, socket, .. } | R::UdpRecv { net, socket, .. } => {
                let (rules, _) = self.net_grant(*net)?;
                if !matches!(self.handles.rows.get(socket), Some(crate::handles::Grant::UdpSocket { net: owner }) if owner == net)
                {
                    return Err(AdapterError::InvalidHandle(*socket));
                }
                match request {
                    R::UdpSend { bytes, .. } if bytes.len() > 65507 => Some(NetError::InvalidInput),
                    R::UdpRecv { max: 0, .. } => Some(NetError::InvalidInput),
                    R::UdpSend { addr, .. }
                        if permits(
                            &rules,
                            &ip_literal(addr.ip),
                            addr.port,
                            Some(NetOperation::UdpPeer),
                        )
                        .is_none() =>
                    {
                        Some(NetError::NotGranted)
                    }
                    _ => None,
                }
            }

            R::UnixConnect { net, path } | R::UnixListen { net, path } => {
                let (rules, _) = self.net_grant(*net)?;
                let operation = if matches!(request, R::UnixConnect { .. }) {
                    NetOperation::UnixConnect
                } else {
                    NetOperation::UnixListen
                };
                if !hirsel_run_schema::canonical_unix_path(path) {
                    Some(NetError::InvalidInput)
                } else {
                    permits(&rules, path, 0, Some(operation))
                        .is_none()
                        .then_some(NetError::NotGranted)
                }
            }
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
            R::NetSplit { net, stream } => {
                self.stream_grant(*net, *stream)?;
                if !matches!(
                    self.handles.rows.get(stream),
                    Some(crate::handles::Grant::Stream { .. })
                ) {
                    return Err(AdapterError::InvalidHandle(*stream));
                }
                None
            }
            R::NetJoin { net, read, write } => {
                let (reading, false) = self.half(*net, *read)? else {
                    return Err(AdapterError::InvalidHandle(*read));
                };
                let (writing, true) = self.half(*net, *write)? else {
                    return Err(AdapterError::InvalidHandle(*write));
                };
                (reading != writing).then_some(NetError::InvalidInput)
            }
            R::NetRead { net, stream, .. }
            | R::NetWrite { net, stream, .. }
            | R::NetWriteAll { net, stream, .. }
            | R::Shutdown { net, stream, .. }
            | R::SetTimeout { net, stream, .. }
            | R::PeerAddr { net, stream }
            | R::TlsPeer { net, stream }
            | R::TlsProtocol { net, stream }
            | R::TlsConnect { net, stream, .. }
            | R::TlsAccept { net, stream, .. }
            | R::TlsClose { net, stream } => {
                let grant = self.stream_grant(*net, *stream)?;
                self.stream_error(request, *stream, &grant)?
            }
            R::Accept { net, listener } | R::LocalAddr { net, listener } => {
                self.listener_grant(*net, *listener)?;
                None
            }
            _ => None,
        })
    }
    /// One stream authority/argument check, shared by the owned and borrowed paths.
    fn stream_error(
        &self,
        request: &crate::io::Request,
        stream: u64,
        grant: &StreamGrant<'_>,
    ) -> Result<Option<NetError>, AdapterError> {
        use crate::io::Request as R;
        match request {
            R::NetRead { max, .. } => return self.stream_io_error(stream, grant, false, *max == 0),
            R::NetWrite { .. } | R::NetWriteAll { .. } => {
                return self.stream_io_error(stream, grant, true, false);
            }
            _ => {}
        }
        if !self.half_serves(request, stream) {
            return Err(AdapterError::InvalidHandle(stream));
        }
        let invalid_tls = match request {
            R::TlsConnect {
                server_name,
                roots,
                identity,
                alpn,
                ..
            } => {
                !hirsel_run_schema::tls::alpn_valid(alpn)
                    || identity.is_some_and(|digest| {
                        self.tls_identities
                            .get(&stream)
                            .is_none_or(|known| known.chain != digest || known.key.is_none())
                    })
                    || !self.tls().valid_name(server_name)
                    || match roots {
                        RootsDigest::WebPki(version) => version != crate::net_tls::WEBPKI_VERSION,
                        RootsDigest::Pem(digest) => {
                            self.pem_validity.get(&stream) != Some(&(*digest, true))
                        }
                    }
            }
            R::TlsAccept { server, .. } => self
                .tls_servers
                .get(&stream)
                .is_none_or(|known| known.digest != *server || known.loaded.is_none()),
            _ => false,
        };
        // A TLS-only rule's stream carries no plaintext before its handshake.
        let plaintext = !grant.tls
            && matches!(
                request,
                R::NetRead { .. } | R::NetWrite { .. } | R::NetWriteAll { .. } | R::Shutdown { .. }
            );
        let denied = match grant.operation {
            NetOperation::ConnectTls => {
                plaintext
                    || matches!(request, R::TlsConnect { server_name, .. } if *server_name != grant.address)
            }
            NetOperation::ListenTls => plaintext || matches!(request, R::TlsConnect { .. }),
            // Streams never carry a UDP rule.
            NetOperation::Connect
            | NetOperation::Listen
            | NetOperation::UdpBind
            | NetOperation::UdpPeer
            | NetOperation::UnixConnect
            | NetOperation::UnixListen
            | NetOperation::UnixListenRemoveStale => false,
        } || matches!(request, R::TlsAccept { .. })
            && !matches!(
                grant.operation,
                NetOperation::Listen
                    | NetOperation::ListenTls
                    | NetOperation::UnixListen
                    | NetOperation::UnixListenRemoveStale
            );
        Ok(
            if invalid_tls
                || matches!(request, R::NetRead { max: 0, .. })
                || matches!(request, R::SetTimeout { timeout, .. } if matches!(timeout, NetTimeout::Nanos(0)))
                || matches!(request, R::TlsConnect { .. } | R::TlsAccept { .. } if grant.tls)
            {
                Some(NetError::InvalidInput)
            } else if denied {
                Some(NetError::NotGranted)
            } else {
                None
            },
        )
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
            if let R::TlsConnect { stream, .. } | R::TlsAccept { stream, .. } = request {
                let closed = self.execute_net(host, &R::Close { handle: *stream })?;
                if matches!(closed, C::Checked(false)) {
                    return Ok(closed);
                }
            }
            // Mismatched halves are consumed as if dropped.
            if let R::NetJoin { read, write, .. } = request {
                for handle in [*read, *write] {
                    let closed = self.execute_net(host, &R::Close { handle })?;
                    if matches!(closed, C::Checked(false)) {
                        return Ok(closed);
                    }
                }
            }
            return Ok(crate::io_checks::net_completion(request, error));
        }
        match request {
            R::UdpBind { net, addr } => {
                let (_, world) = self.net_grant(*net)?;
                let id = self.handles.next();
                let value = provided(self.world(world, host)?.udp_bind(id, *addr))?;
                Ok(C::NetHandle(value.map(|()| {
                    self.handles.insert(Grant::UdpSocket { net: *net })
                })))
            }
            R::UdpSend {
                net,
                socket,
                bytes,
                addr,
            } => {
                let (_, world) = self.net_grant(*net)?;
                let value = provided(self.world(world, host)?.udp_send(*socket, bytes, *addr))?;
                if value.as_ref().is_ok_and(|n| *n != bytes.len() as u64) {
                    return Err(AdapterError::InvalidCompletion);
                }
                Ok(C::Count(value))
            }
            R::UdpRecv { net, socket, max } => {
                let (rules, world) = self.net_grant(*net)?;
                // A receive loop drops refused datagrams while the world is borrowed mutably.
                let rules = rules.clone();
                loop {
                    let value = provided(self.world(world, host)?.udp_recv(*socket))?;
                    match value {
                        Err(e) => return Ok(C::Datagram(Err(e))),
                        Ok((mut bytes, peer)) => {
                            if permits(
                                &rules,
                                &ip_literal(peer.ip),
                                peer.port,
                                Some(NetOperation::UdpPeer),
                            )
                            .is_none()
                            {
                                continue;
                            }
                            let truncated = bytes.len() as u64 > *max;
                            bytes.truncate((*max).min(bytes.len() as u64) as usize);
                            return Ok(C::Datagram(Ok((bytes, peer, truncated))));
                        }
                    }
                }
            }
            R::TestNet {
                test,
                peers,
                listeners,
            } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                let script = (crate::runtime_roots::script()
                    .ok_or_else(|| {
                        AdapterError::Unsupported("scripted network provider is not linked".into())
                    })?
                    .new)(peers.clone(), listeners.clone())?;
                let rules = peers
                    .iter()
                    .map(|peer| NetRule {
                        address: peer.address.clone(),
                        port: peer.port,
                        operation: match &peer.tls {
                            _ if hirsel_run_schema::canonical_unix_path(&peer.address) => {
                                NetOperation::UnixConnect
                            }
                            hirsel_run_schema::PeerTls::Plain => NetOperation::Connect,
                            // A client's Presents never names a checked peer.
                            hirsel_run_schema::PeerTls::Certificate(_)
                            | hirsel_run_schema::PeerTls::Fails(_)
                            | hirsel_run_schema::PeerTls::Presents(..)
                            | hirsel_run_schema::PeerTls::Offers(..)
                            | hirsel_run_schema::PeerTls::Serves(_) => NetOperation::ConnectTls,
                        },
                    })
                    .chain(listeners.iter().map(|listener| NetRule {
                        address: listener.address.clone(),
                        port: listener.port,
                        operation: if hirsel_run_schema::canonical_unix_path(&listener.address) {
                            NetOperation::UnixListen
                        } else {
                            NetOperation::Listen
                        },
                    }))
                    .chain(
                        peers
                            .iter()
                            .filter(|p| {
                                matches!(p.tls, hirsel_run_schema::PeerTls::Plain)
                                    && p.address.parse::<std::net::IpAddr>().is_ok()
                            })
                            .map(|p| NetRule {
                                address: p.address.clone(),
                                port: p.port,
                                operation: NetOperation::UdpPeer,
                            }),
                    )
                    .chain(
                        listeners
                            .iter()
                            .filter(|l| l.address.parse::<std::net::IpAddr>().is_ok())
                            .map(|l| NetRule {
                                address: l.address.clone(),
                                port: l.port,
                                operation: NetOperation::UdpBind,
                            }),
                    )
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
                        protocol: None,
                    })
                })))
            }
            R::UnixConnect { net, path } => {
                let (_, world) = self.net_grant(*net)?;
                let id = self.handles.next();
                let value = provided(self.world(world, host)?.unix_connect(id, path))?;
                Ok(C::NetHandle(value.map(|()| {
                    self.handles.insert(Grant::Stream {
                        net: *net,
                        address: path.clone(),
                        port: 0,
                        operation: NetOperation::UnixConnect,
                        tls: false,
                        protocol: None,
                    })
                })))
            }
            R::UnixListen { net, path } => {
                let (rules, world) = self.net_grant(*net)?;
                let operation = permits(&rules, path, 0, Some(NetOperation::UnixListen)).unwrap();
                let remove_stale = operation == NetOperation::UnixListenRemoveStale;
                let id = self.handles.next();
                let value = provided(self.world(world, host)?.unix_listen(id, path, remove_stale))?;
                Ok(C::Listened(value.map(|()| {
                    let id = self.handles.insert(Grant::Listener {
                        net: *net,
                        operation,
                    });
                    (id, SocketAddr::UNNAMED)
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
                        protocol: None,
                    })
                })))
            }
            R::NetRead { net, stream, .. }
            | R::NetWrite { net, stream, .. }
            | R::NetWriteAll { net, stream, .. }
            | R::Shutdown { net, stream, .. }
            | R::SetTimeout { net, stream, .. }
            | R::PeerAddr { net, stream }
            | R::TlsPeer { net, stream } => {
                let grant = self.stream_grant(*net, *stream)?;
                let (id, world) = (grant.id, grant.world);
                let stream = &id;
                let provider = self.world(world, host)?;
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
                    R::TlsPeer { .. } => C::Certificates(provided(provider.tls_peer(*stream))?),
                    _ => unreachable!(),
                };
                // A write that contradicts a scripted client's Expect fails LANG-9 after
                // its Test report (docs/spec/core-library.md, scripted peers).
                if let NetWorld::Script(world) = world
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
            R::NetSplit { net, stream } => {
                // The connection's grant waits aside for its halves; no provider is consulted.
                let grant = self
                    .handles
                    .rows
                    .remove(stream)
                    .ok_or(AdapterError::InvalidHandle(*stream))?;
                self.split_streams.insert(*stream, (grant, 2));
                let half = |write| Grant::Half {
                    net: *net,
                    stream: *stream,
                    write,
                };
                let read = self.handles.insert(half(false));
                let write = self.handles.insert(half(true));
                Ok(C::Halves(Ok((read, write))))
            }
            R::NetJoin { read, write, .. } => {
                let Some(Grant::Half { stream, .. }) = self.handles.rows.remove(read) else {
                    return Err(AdapterError::InvalidHandle(*read));
                };
                self.handles.rows.remove(write);
                let (grant, _) = self
                    .split_streams
                    .remove(&stream)
                    .ok_or(AdapterError::InvalidHandle(stream))?;
                self.handles.rows.insert(stream, grant);
                Ok(C::NetHandle(Ok(stream)))
            }
            R::TlsProtocol { net, stream } => {
                // Handshake state only: no provider is consulted.
                self.stream_grant(*net, *stream)?;
                match self.handles.rows.get(stream) {
                    Some(Grant::Stream { protocol, .. }) => Ok(C::Protocol(Ok(protocol.clone()))),
                    _ => Err(AdapterError::InvalidHandle(*stream)),
                }
            }
            R::TlsConnect {
                net,
                stream,
                server_name,
                roots,
                identity,
                alpn,
            } => {
                let grant = self.stream_grant(*net, *stream)?;
                let key = match identity {
                    Some(_) => Some(
                        self.tls_identities
                            .get(stream)
                            .and_then(|identity| identity.key.clone())
                            .ok_or(AdapterError::InvalidCompletion)?,
                    ),
                    None => None,
                };
                // The stream is taken: it is closed on every failure.
                let tls = self.tls();
                let value = provided(self.world(grant.world, host)?.tls_connect(
                    tls,
                    *stream,
                    server_name,
                    roots,
                    key.as_ref(),
                    alpn,
                ))?;
                self.pem_validity.remove(stream);
                self.tls_identities.remove(stream);
                // A server selects only a protocol the client offered.
                if let Ok(Some(protocol)) = &value
                    && !alpn.contains(protocol)
                {
                    return Err(AdapterError::InvalidCompletion);
                }
                match &value {
                    Ok(protocol) => self.mark_tls(*stream, protocol.clone()),
                    Err(_) => {
                        let closed = self.execute_net(host, &R::Close { handle: *stream })?;
                        if matches!(closed, C::Checked(false)) {
                            return Ok(closed);
                        }
                    }
                }
                Ok(C::Protocol(value))
            }
            R::TlsAccept { net, stream, .. } => {
                let grant = self.stream_grant(*net, *stream)?;
                let server = self
                    .tls_servers
                    .get(stream)
                    .and_then(|server| server.loaded.clone())
                    .ok_or(AdapterError::InvalidCompletion)?;
                // The stream is taken: it is closed on every failure. A parked handshake
                // keeps its server for the re-issued request.
                let tls = self.tls();
                let value = provided(
                    self.world(grant.world, host)?
                        .tls_accept(tls, *stream, &server),
                )?;
                self.tls_servers.remove(stream);
                // A server selects only from its own list.
                if let Ok(Some(protocol)) = &value
                    && !server.alpn.contains(protocol)
                {
                    return Err(AdapterError::InvalidCompletion);
                }
                match &value {
                    Ok(protocol) => self.mark_tls(*stream, protocol.clone()),
                    Err(_) => {
                        let closed = self.execute_net(host, &R::Close { handle: *stream })?;
                        if matches!(closed, C::Checked(false)) {
                            return Ok(closed);
                        }
                    }
                }
                Ok(C::Protocol(value))
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
                let Some(operation) =
                    permits(&rules, &literal, addr.port, Some(NetOperation::Listen))
                else {
                    return Ok(C::Listened(Err(NetError::NotGranted)));
                };
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
                let id = self.handles.insert(Grant::Listener {
                    net: *net,
                    operation,
                });
                Ok(C::Listened(Ok((id, local))))
            }
            R::Accept { net, listener } => {
                let (world, operation) = self.listener_grant(*net, *listener)?;
                let id = self.handles.next();
                let value = provided(self.world(world, host)?.accept(*listener, id))?;
                // Accepted peers are not checked: the listen rule is the authority, and the
                // stream keeps its operation.
                Ok(C::Accepted(value.map(|peer| {
                    let id = self.handles.insert(Grant::Stream {
                        net: *net,
                        address: ip_literal(peer.ip),
                        port: peer.port,
                        operation,
                        tls: false,
                        protocol: None,
                    });
                    (id, peer)
                })))
            }
            R::LocalAddr { net, listener } => {
                let (world, _) = self.listener_grant(*net, *listener)?;
                Ok(C::Address(provided(
                    self.world(world, host)?.local_addr(*listener),
                )?))
            }
            R::Close { handle: half }
                if matches!(self.handles.rows.get(half), Some(Grant::Half { .. })) =>
            {
                // A half's drop has no network effect; the last one closes its connection.
                let Some(Grant::Half { stream, .. }) = self.handles.rows.remove(half) else {
                    unreachable!()
                };
                let live = &mut self
                    .split_streams
                    .get_mut(&stream)
                    .ok_or(AdapterError::InvalidHandle(stream))?
                    .1;
                *live -= 1;
                if *live > 0 {
                    return Ok(C::Unit(Ok(())));
                }
                let (grant, _) = self
                    .split_streams
                    .remove(&stream)
                    .ok_or(AdapterError::InvalidHandle(stream))?;
                self.handles.rows.insert(stream, grant);
                self.execute_net(host, &R::Close { handle: stream })
            }
            R::Close { handle } => {
                self.pem_validity.remove(handle);
                self.tls_identities.remove(handle);
                self.tls_servers.remove(handle);
                let world = match self.handles.rows.remove(handle) {
                    Some(
                        Grant::Stream { net, .. }
                        | Grant::Listener { net, .. }
                        | Grant::UdpSocket { net },
                    ) => self.net_grant(net)?.1,
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
