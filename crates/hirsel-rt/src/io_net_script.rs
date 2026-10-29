//! Independent native implementation of the scripted network peers.
//! This is Hirsel's peer language, not a ported TCP stack. Rust std's
//! library/std/src/net/tcp.rs supplies read/timeout/half-close conventions;
//! reference vectors and script vectors live in tests/it/net_script.rs.
#[unsafe(export_name = "hirsel.runtime.script.provider")]
static PROVIDER: crate::runtime_roots::Script = crate::runtime_roots::Script {
    new: ScriptNet::new,
};

use super::{
    io::AdapterError,
    net::{
        NetError, NetProvider, NetTimeout, Provided, RootsDigest, ShutdownHow, SocketAddr,
        TimeoutKind, TlsFailure,
    },
};
use hirsel_run_schema::{NetConnection, NetListener, NetPeer, NetStep, PeerTls};
use std::{collections::BTreeMap, net::IpAddr};

/// A listen address as scripted listeners name it: an IP literal and port, or a
/// socket path with port 0.
type Endpoint = (String, u16);

/// Who is at the other end: a scripted peer the program connected to, or the
/// scripted client a listener accepted.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Remote {
    Peer(usize),
    Client {
        index: usize,
        listener: Endpoint,
        from: SocketAddr,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Connection {
    remote: Remote,
    tls: bool,
    /// What `tls.peer` returns: the chain the handshake's peer presented.
    peer: Option<Vec<u8>>,
    steps: Vec<NetStep>,
    step: usize,
    offset: usize,
    written: Vec<u8>,
    consumed: usize,
    reset: bool,
    read_timeout: bool,
    read_closed: bool,
    write_closed: bool,
    /// A scripted client's failed Expect: the Test report of the LANG-9 failure.
    mismatch: Option<String>,
}
impl Connection {
    fn new(remote: Remote, steps: Vec<NetStep>) -> Self {
        Self {
            remote,
            tls: false,
            peer: None,
            steps,
            step: 0,
            offset: 0,
            written: vec![],
            consumed: 0,
            reset: false,
            read_timeout: false,
            read_closed: false,
            write_closed: false,
            mismatch: None,
        }
    }
    fn progress(&mut self) {
        while let Some(step) = self.steps.get(self.step) {
            match step {
                NetStep::Expect(expected)
                    if self.written.len() - self.consumed >= expected.len() =>
                {
                    let wrote = &self.written[self.consumed..][..expected.len()];
                    if wrote != *expected {
                        if let Remote::Client {
                            index, listener, ..
                        } = &self.remote
                        {
                            self.mismatch = Some(hirsel_run_schema::net_client_mismatch(
                                *index,
                                &hirsel_run_schema::listener_name(&listener.0, listener.1),
                                expected,
                                wrote,
                            ));
                        }
                        self.reset = true;
                        return;
                    }
                    self.consumed += expected.len();
                }
                NetStep::Send(bytes) if self.offset == bytes.len() => {
                    self.offset = 0;
                }
                NetStep::Reset => {
                    self.reset = true;
                    return;
                }
                _ => return,
            }
            self.step += 1;
        }
    }
    fn check(&mut self) -> Result<(), NetError> {
        self.progress();
        if self.reset {
            Err(NetError::ConnectionReset)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptNet {
    peers: Vec<NetPeer>,
    next_port: u32,
    next_connection: Vec<usize>,
    listeners: Vec<NetListener>,
    /// The next client of each scripted listener address.
    next_client: Vec<usize>,
    open_listeners: BTreeMap<u64, Endpoint>,
    streams: BTreeMap<u64, Connection>,
    udp: BTreeMap<u64, UdpScript>,
}
impl ScriptNet {
    pub fn new(peers: Vec<NetPeer>, listeners: Vec<NetListener>) -> Result<Self, AdapterError> {
        hirsel_run_schema::check_net_listeners(&listeners).map_err(AdapterError::Unsupported)?;
        let mut endpoints = BTreeMap::new();
        for (index, p) in peers.iter().enumerate() {
            if let Some(first) = endpoints.insert((&p.address, p.port), index) {
                return Err(AdapterError::Unsupported(format!(
                    "duplicate address/port at entries {first} and {index}"
                )));
            }
            if hirsel_run_schema::check_peer_address(p).is_err()
                || p.resolves_to.iter().any(|a| {
                    a.parse::<IpAddr>().is_err()
                        || a.parse::<IpAddr>().is_ok_and(|ip| ip.to_string() != *a)
                })
            {
                return Err(AdapterError::Unsupported(
                    "noncanonical scripted peer address".into(),
                ));
            }
        }
        Ok(Self {
            next_connection: vec![0; peers.len()],
            peers,
            next_port: 49152,
            next_client: vec![0; listeners.len()],
            listeners,
            open_listeners: BTreeMap::new(),
            streams: BTreeMap::new(),
            udp: BTreeMap::new(),
        })
    }
    /// The Test report of closing `handle` while a scripted client it leaves still
    /// expects bytes: an accepted stream short of its Expect, or a listener with an
    /// unaccepted client that has one (docs/spec/core-library.md, scripted peers).
    pub(crate) fn unmet(&mut self, handle: u64) -> Option<String> {
        if let Some(s) = self.streams.get_mut(&handle) {
            let Remote::Client {
                index, listener, ..
            } = s.remote.clone()
            else {
                return None;
            };
            s.progress();
            if s.reset {
                return None;
            }
            let expected = s.steps[s.step..]
                .iter()
                .take_while(|step| !matches!(step, NetStep::Reset))
                .find_map(|step| match step {
                    NetStep::Expect(b) => Some(b),
                    _ => None,
                })?;
            return Some(hirsel_run_schema::net_client_unmet(
                index,
                &hirsel_run_schema::listener_name(&listener.0, listener.1),
                expected,
                &s.written[s.consumed..],
            ));
        }
        let (address, port) = self.open_listeners.get(&handle)?;
        let i = self
            .listeners
            .iter()
            .position(|l| l.address == *address && l.port == *port)?;
        let client =
            hirsel_run_schema::first_expecting_client(&self.listeners[i], self.next_client[i])?;
        Some(hirsel_run_schema::net_client_unaccepted(
            client,
            &hirsel_run_schema::listener_name(address, *port),
        ))
    }
    /// The Test report of the write that just contradicted a scripted client's Expect.
    pub(crate) fn take_mismatch(&mut self, stream: u64) -> Option<String> {
        self.streams.get_mut(&stream)?.mismatch.take()
    }
    fn connection(&mut self, handle: u64) -> Result<&mut Connection, AdapterError> {
        self.streams
            .get_mut(&handle)
            .ok_or(AdapterError::InvalidHandle(handle))
    }
    fn open(&mut self, id: u64, host: &str, port: u16, timeout: NetTimeout) -> Provided<()> {
        if matches!(timeout, NetTimeout::Nanos(0)) {
            return Ok(Err(NetError::InvalidInput));
        }
        if self.streams.contains_key(&id) {
            return Err(AdapterError::InvalidHandle(id));
        }
        let Some(i) = self
            .peers
            .iter()
            .position(|p| p.address == host && p.port == port)
        else {
            return Ok(Err(NetError::ConnectionRefused));
        };
        let entry = self.peers[i].connections.get(self.next_connection[i]);
        self.next_connection[i] += 1;
        match entry {
            Some(NetConnection::Open(steps)) => {
                self.streams
                    .insert(id, Connection::new(Remote::Peer(i), steps.clone()));
                Ok(Ok(()))
            }
            Some(NetConnection::Unanswered) => {
                if matches!(timeout, NetTimeout::Unbounded) {
                    Err(AdapterError::Blocked)
                } else {
                    Ok(Err(NetError::TimedOut))
                }
            }
            Some(NetConnection::Refused) | None => Ok(Err(NetError::ConnectionRefused)),
        }
    }
}
impl ScriptNet {
    /// A second listen on an open listener's address is `AddrInUse()`.
    fn bind_listener(&mut self, id: u64, endpoint: Endpoint) -> Provided<()> {
        if self.open_listeners.values().any(|open| *open == endpoint) {
            return Ok(Err(NetError::AddrInUse));
        }
        self.open_listeners.insert(id, endpoint);
        Ok(Ok(()))
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct UdpScript {
    local: SocketAddr,
    peers: Vec<(SocketAddr, std::collections::VecDeque<NetStep>)>,
    received: std::collections::VecDeque<(Vec<u8>, SocketAddr)>,
}
impl NetProvider for ScriptNet {
    fn udp_bind(&mut self, id: u64, mut addr: SocketAddr) -> Provided<()> {
        if addr.port == 0 {
            while self.next_port <= u16::MAX as u32
                && self.udp.values().any(|s| {
                    s.local
                        == SocketAddr {
                            port: self.next_port as u16,
                            ..addr
                        }
                })
            {
                self.next_port += 1;
            }
            if self.next_port > u16::MAX as u32 {
                return Ok(Err(NetError::AddrNotAvailable));
            }
            addr.port = self.next_port as u16;
            self.next_port += 1;
        }
        if self.udp.values().any(|s| s.local == addr) {
            return Ok(Err(NetError::AddrInUse));
        }
        let peers = self
            .peers
            .iter()
            .filter_map(|p| {
                if !matches!(p.tls, PeerTls::Plain) {
                    return None;
                }
                let ip = p.address.parse().ok()?;
                let steps = match p.connections.first()? {
                    NetConnection::Open(steps) => steps.clone(),
                    NetConnection::Refused => vec![NetStep::Reset],
                    NetConnection::Unanswered => vec![NetStep::Stall],
                };
                Some((SocketAddr { ip, port: p.port }, steps.into()))
            })
            .collect();
        self.udp.insert(
            id,
            UdpScript {
                local: addr,
                peers,
                received: Default::default(),
            },
        );
        Ok(Ok(()))
    }
    fn udp_send(&mut self, socket: u64, bytes: &[u8], addr: SocketAddr) -> Provided<u64> {
        let local = self.udp[&socket].local;
        if let Some(target) = self.udp.values_mut().find(|s| s.local == addr) {
            target.received.push_back((bytes.to_vec(), local));
        } else if let Some((_, steps)) = self
            .udp
            .get_mut(&socket)
            .unwrap()
            .peers
            .iter_mut()
            .find(|(a, _)| *a == addr)
        {
            match steps.front() {
                Some(NetStep::Expect(expected)) if expected == bytes => {
                    steps.pop_front();
                }
                Some(NetStep::Expect(_) | NetStep::Reset) => {
                    return Ok(Err(NetError::ConnectionReset));
                }
                _ => {}
            }
        }
        Ok(Ok(bytes.len() as u64))
    }
    fn udp_recv(&mut self, socket: u64) -> Provided<(Vec<u8>, SocketAddr)> {
        let socket = self.udp.get_mut(&socket).unwrap();
        if let Some(datagram) = socket.received.pop_front() {
            return Ok(Ok(datagram));
        }
        for (peer, steps) in &mut socket.peers {
            match steps.front() {
                Some(NetStep::Send(_)) => {
                    let Some(NetStep::Send(bytes)) = steps.pop_front() else {
                        unreachable!()
                    };
                    return Ok(Ok((bytes, *peer)));
                }
                Some(NetStep::Reset) => return Ok(Err(NetError::ConnectionReset)),
                _ => {}
            }
        }
        Err(AdapterError::Blocked)
    }

    fn resolve(&mut self, host: &str, port: u16) -> Provided<Vec<SocketAddr>> {
        let Some(peer) = self
            .peers
            .iter()
            .find(|p| p.address == host && p.port == port)
        else {
            return Ok(Err(NetError::UnknownHost));
        };
        Ok(Ok(peer
            .resolves_to
            .iter()
            .map(|ip| SocketAddr {
                ip: ip.parse().expect("validated script IP"),
                port,
            })
            .collect()))
    }
    fn connect(&mut self, id: u64, host: &str, port: u16, timeout: NetTimeout) -> Provided<()> {
        self.open(id, host, port, timeout)
    }
    fn connect_addr(&mut self, id: u64, addr: SocketAddr, timeout: NetTimeout) -> Provided<()> {
        self.open(id, &addr.ip.to_string(), addr.port, timeout)
    }
    fn read(&mut self, id: u64, max: u64) -> Provided<Vec<u8>> {
        if max == 0 {
            return Ok(Err(NetError::InvalidInput));
        }
        let s = self.connection(id)?;
        if let Err(e) = s.check() {
            return Ok(Err(e));
        }
        if s.read_closed {
            return Ok(Ok(vec![]));
        }
        match s.steps.get(s.step) {
            Some(NetStep::Send(bytes)) => {
                let count =
                    (bytes.len() - s.offset).min(usize::try_from(max).unwrap_or(usize::MAX));
                let result = bytes[s.offset..s.offset + count].to_vec();
                s.offset += count;
                Ok(Ok(result))
            }
            None | Some(NetStep::Close) => Ok(Ok(vec![])),
            // Only a secured stream has a close_notify to miss.
            Some(NetStep::Truncate) if s.tls => Ok(Err(NetError::Tls(TlsFailure::Truncated))),
            Some(NetStep::Truncate) => Ok(Ok(vec![])),
            _ if s.read_timeout => Ok(Err(NetError::TimedOut)),
            _ => Err(AdapterError::Blocked),
        }
    }
    fn write(&mut self, id: u64, bytes: &[u8]) -> Provided<u64> {
        let s = self.connection(id)?;
        if let Err(e) = s.check() {
            return Ok(Err(e));
        }
        if s.write_closed {
            return Ok(Err(NetError::BrokenPipe));
        }
        s.written.extend_from_slice(bytes);
        s.progress();
        Ok(Ok(bytes.len() as u64))
    }
    fn write_all(&mut self, id: u64, bytes: &[u8]) -> Provided<()> {
        Ok(self.write(id, bytes)?.map(|_| ()))
    }
    fn shutdown(&mut self, id: u64, how: ShutdownHow) -> Provided<()> {
        let s = self.connection(id)?;
        if let Err(e) = s.check() {
            return Ok(Err(e));
        }
        if matches!(how, ShutdownHow::Read | ShutdownHow::Both) {
            s.read_closed = true;
        }
        if matches!(how, ShutdownHow::Write | ShutdownHow::Both) {
            s.write_closed = true;
        }
        Ok(Ok(()))
    }
    fn set_timeout(&mut self, id: u64, which: TimeoutKind, timeout: NetTimeout) -> Provided<()> {
        if matches!(timeout, NetTimeout::Nanos(0)) {
            return Ok(Err(NetError::InvalidInput));
        }
        let s = self.connection(id)?;
        if let Err(e) = s.check() {
            return Ok(Err(e));
        }
        if matches!(which, TimeoutKind::Read) {
            s.read_timeout = !matches!(timeout, NetTimeout::Unbounded);
        }
        Ok(Ok(()))
    }
    fn peer_addr(&mut self, id: u64) -> Provided<SocketAddr> {
        let s = self.connection(id)?;
        if let Err(e) = s.check() {
            return Ok(Err(e));
        }
        let peer_index = match s.remote {
            Remote::Peer(i) => i,
            Remote::Client { from, .. } => return Ok(Ok(from)),
        };
        let peer = &self.peers[peer_index];
        if hirsel_run_schema::canonical_unix_path(&peer.address) {
            return Ok(Ok(SocketAddr::UNNAMED));
        }
        let ip = peer
            .address
            .parse::<IpAddr>()
            .ok()
            .or_else(|| peer.resolves_to.first()?.parse().ok());
        Ok(ip
            .map(|ip| SocketAddr {
                ip,
                port: peer.port,
            })
            .ok_or(NetError::UnknownHost))
    }
    /// Only a `Serves` peer asks for `identity`; `roots` is never consulted.
    fn tls_connect(
        &mut self,
        tls: &dyn crate::net_tls::TlsEngine,
        id: u64,
        server_name: &str,
        _roots: &RootsDigest,
        identity: Option<&std::sync::Arc<hirsel_run_schema::tls::Identity>>,
        alpn: &[String],
    ) -> Provided<Option<String>> {
        let s = self.connection(id)?;
        // Owner 385145: upgrading an already secured Stream consumes it with
        // InvalidInput, just as the host rustls provider does. State closes it.
        if s.tls {
            return Ok(Err(NetError::InvalidInput));
        }
        if let Err(e) = s.check() {
            return Ok(Err(e));
        }
        if matches!(s.steps.get(s.step), Some(NetStep::Stall)) {
            return if s.read_timeout {
                Ok(Err(NetError::TimedOut))
            } else {
                Err(AdapterError::Blocked)
            };
        }
        // An accepted stream has no TLS: its client is a plain peer.
        let Remote::Peer(peer_index) = s.remote else {
            return Ok(Err(NetError::Tls(TlsFailure::HandshakeFailure)));
        };
        let verdict = tls.scripted_connect(
            &self.peers[peer_index].tls,
            server_name,
            identity.map(|identity| &**identity),
            alpn,
        );
        Ok(match verdict {
            Ok((peer, protocol)) => {
                let s = self.connection(id)?;
                s.tls = true;
                s.peer = peer;
                Ok(protocol)
            }
            Err(failure) => Err(NetError::Tls(failure.into())),
        })
    }
    fn tls_accept(
        &mut self,
        tls: &dyn crate::net_tls::TlsEngine,
        id: u64,
        server: &hirsel_run_schema::tls::Server,
    ) -> Provided<Option<String>> {
        let s = self.connection(id)?;
        if s.tls {
            return Ok(Err(NetError::InvalidInput));
        }
        if let Err(e) = s.check() {
            return Ok(Err(e));
        }
        if matches!(s.steps.get(s.step), Some(NetStep::Stall)) {
            return if s.read_timeout {
                Ok(Err(NetError::TimedOut))
            } else {
                Err(AdapterError::Blocked)
            };
        }
        // Policy admits only accepted streams.
        let Remote::Client {
            index, listener, ..
        } = s.remote.clone()
        else {
            return Err(AdapterError::InvalidCompletion);
        };
        let client = self
            .listeners
            .iter()
            .find(|l| l.address == listener.0 && l.port == listener.1)
            .and_then(|l| l.clients.get(index))
            .ok_or(AdapterError::InvalidCompletion)?;
        let verdict = tls.scripted_accept(&client.tls, server);
        Ok(match verdict {
            Ok((peer, protocol)) => {
                let s = self.connection(id)?;
                s.tls = true;
                s.peer = peer;
                Ok(protocol)
            }
            Err(failure) => Err(NetError::Tls(failure.into())),
        })
    }
    fn tls_peer(&mut self, id: u64) -> Provided<Option<Vec<u8>>> {
        Ok(Ok(self.connection(id)?.peer.clone()))
    }
    fn tls_close(&mut self, id: u64) -> Provided<()> {
        let s = self.connection(id)?;
        Ok(if s.tls { s.check() } else { Ok(()) })
    }
    fn listen(&mut self, id: u64, mut addr: SocketAddr) -> Provided<()> {
        if self.open_listeners.contains_key(&id) || self.streams.contains_key(&id) {
            return Err(AdapterError::InvalidHandle(id));
        }
        if addr.port == 0 {
            while self.next_port <= u32::from(u16::MAX)
                && (self
                    .listeners
                    .iter()
                    .any(|l| u32::from(l.port) == self.next_port)
                    || self
                        .peers
                        .iter()
                        .any(|p| u32::from(p.port) == self.next_port)
                    || self
                        .open_listeners
                        .values()
                        .any(|a| u32::from(a.1) == self.next_port))
            {
                self.next_port += 1;
            }
            let Ok(port) = u16::try_from(self.next_port) else {
                return Ok(Err(NetError::AddrInUse));
            };
            self.next_port += 1;
            addr.port = port;
            if let Some(template) = self
                .listeners
                .iter_mut()
                .find(|l| l.port == 0 && l.address == addr.ip.to_string())
            {
                template.port = port;
            }
        }
        self.bind_listener(id, (addr.ip.to_string(), addr.port))
    }
    fn unix_connect(&mut self, id: u64, path: &str) -> Provided<()> {
        self.open(id, path, 0, NetTimeout::Unbounded)
    }
    fn unix_listen(&mut self, id: u64, path: &str, _remove_stale: bool) -> Provided<()> {
        // A scripted world has no socket files, so there is never a stale one to remove.
        if self.open_listeners.contains_key(&id) || self.streams.contains_key(&id) {
            return Err(AdapterError::InvalidHandle(id));
        }
        self.bind_listener(id, (path.to_owned(), 0))
    }
    fn accept(&mut self, listener: u64, id: u64) -> Provided<SocketAddr> {
        let addr = self
            .open_listeners
            .get(&listener)
            .cloned()
            .ok_or(AdapterError::InvalidHandle(listener))?;
        if self.streams.contains_key(&id) {
            return Err(AdapterError::InvalidHandle(id));
        }
        let Some(i) = self
            .listeners
            .iter()
            .position(|l| l.address == addr.0 && l.port == addr.1)
        else {
            return Ok(Err(NetError::ConnectionAborted));
        };
        let index = self.next_client[i];
        let Some(client) = self.listeners[i].clients.get(index) else {
            return Ok(Err(NetError::ConnectionAborted));
        };
        self.next_client[i] += 1;
        let from = SocketAddr {
            ip: client.address.parse().expect("validated client address"),
            port: client.port,
        };
        let remote = Remote::Client {
            index,
            listener: addr,
            from,
        };
        self.streams
            .insert(id, Connection::new(remote, client.steps.clone()));
        Ok(Ok(from))
    }
    fn local_addr(&mut self, listener: u64) -> Provided<SocketAddr> {
        let (address, port) = self
            .open_listeners
            .get(&listener)
            .ok_or(AdapterError::InvalidHandle(listener))?;
        Ok(Ok(address.parse().map_or(SocketAddr::UNNAMED, |ip| {
            SocketAddr { ip, port: *port }
        })))
    }
    fn close(&mut self, id: u64) {
        self.udp.remove(&id);
        self.streams.remove(&id);
        self.open_listeners.remove(&id);
    }
}

/// Cloneable, socket-free network, process and Env capability model for driver-owned
/// execution oracles. Policy, brands, peer transitions and scripted runs are the same
/// State path used by native execution; no runtime allocation helper is involved, and
/// no host program runs.
#[derive(Clone, Debug, Default)]
pub struct ScriptIo {
    state: crate::io::State,
}
impl ScriptIo {
    /// The Test reports the model's calls appended since the last take.
    pub fn take_report(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.state.stdout)
    }
    pub fn new(test_seed: Option<u64>) -> Self {
        let mut model = Self::default();
        if let Some(seed) = test_seed {
            model.state.handles = crate::handles::Handles::test(seed);
        }
        model
    }
    /// A Main launch's admitted `io.net` grant: its rules, and with peers its scripted world,
    /// as a host launch configures them.
    pub fn set_net_grant(
        &mut self,
        grant: &hirsel_run_schema::NetGrant,
    ) -> Result<(), AdapterError> {
        self.state.net_rules = crate::net::NetRule::from_config(grant);
        match &grant.peers {
            Some(peers) => self
                .state
                .set_net_script(peers.clone(), grant.listeners.clone()),
            None => Ok(()),
        }
    }
    /// A Main launch's admitted `io.process` grant; absent, the root Proc runs nothing.
    pub fn set_process_grant(&mut self, grant: &hirsel_run_schema::ProcessGrant) {
        self.state.set_process_grant(grant);
    }
    /// A Main launch's Env snapshot; absent, the root Env grants no name.
    pub fn set_env(&mut self, env: std::collections::BTreeMap<String, Option<String>>) {
        self.state.env = env;
    }
    /// A monotonic clock handle for a `proc.run` deadline to name. Its caller owns the
    /// clock's reading and reports it with [`Self::observe_monotonic`] before each request.
    pub fn monotonic_clock(&mut self) -> u64 {
        self.state
            .handles
            .insert(crate::handles::Grant::MonotonicClock)
    }
    /// The monotonic clock's reading now, which a scripted run's deadline is checked against.
    pub fn observe_monotonic(&mut self, now: u64) {
        self.state.last_monotonic = Some(now);
    }
    pub fn call(
        &mut self,
        request: crate::io::Request,
    ) -> Result<crate::io::Completion, AdapterError> {
        use crate::io::{Request, Tree};
        if !crate::net::is_net_request(&request)
            && !crate::process::is_process_request(&request)
            && !matches!(
                request,
                Request::Net { .. }
                    | Request::Close { .. }
                    | Request::EnvGet { .. }
                    | Request::EnvNarrow { .. }
            )
        {
            return Err(AdapterError::Unsupported("network model request".into()));
        }
        let mut tree = Tree::default();
        let mut identities = crate::io_sim::FileIdentities::new(&tree);
        self.state.execute(
            &mut crate::io_sim::MemoryTree {
                tree: &mut tree,
                identities: &mut identities,
            },
            &mut crate::net::NoNet,
            &request,
            None,
        )
    }
}

impl crate::io::Adapter for ScriptIo {
    fn tls(&mut self, engine: &'static dyn crate::net_tls::TlsEngine) {
        self.state.tls = crate::net_tls::Engine(Some(engine));
    }
    fn tls_roots(&mut self, stream: u64, pem: &[u8]) {
        self.state.remember_tls_roots(stream, pem);
    }
    fn tls_identity(&mut self, stream: u64, chain: &[u8], key: &[u8]) {
        self.state.remember_tls_identity(stream, chain, key);
    }
    fn tls_server(
        &mut self,
        stream: u64,
        identities: &[(&[u8], &[u8])],
        clients: crate::net::ClientAuth<&[u8]>,
        alpn: &[String],
    ) {
        self.state
            .remember_tls_server(stream, identities, clients, alpn);
    }
    fn preflight(
        &self,
        request: &crate::io::Request,
    ) -> Result<Option<crate::io::Completion>, AdapterError> {
        self.state.preflight(request)
    }
    fn call(&mut self, request: crate::io::Request) -> Result<crate::io::Completion, AdapterError> {
        ScriptIo::call(self, request)
    }
}
