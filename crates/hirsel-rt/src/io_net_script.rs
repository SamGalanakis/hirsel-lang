//! Independent native implementation of the scripted network peers.
//! This is Hirsel's peer language, not a ported TCP stack. Rust std's
//! library/std/src/net/tcp.rs supplies read/timeout/half-close conventions;
//! reference vectors and script vectors live in tests/it/net_script.rs.
use super::{
    io::AdapterError,
    net::{
        NetError, NetProvider, NetTimeout, Provided, RootsDigest, ShutdownHow, SocketAddr,
        TimeoutKind, TlsFailure,
    },
};
use hirsel_run_config::{
    NetConnection, NetPeer, NetStep, PeerTls, canonical_net_address as canonical_address,
};
use std::{collections::BTreeMap, net::IpAddr};

#[derive(Clone, Debug, Eq, PartialEq)]
struct Connection {
    peer: usize,
    tls: bool,
    steps: Vec<NetStep>,
    step: usize,
    offset: usize,
    written: Vec<u8>,
    consumed: usize,
    reset: bool,
    read_timeout: bool,
    read_closed: bool,
    write_closed: bool,
}
impl Connection {
    fn progress(&mut self) {
        while let Some(step) = self.steps.get(self.step) {
            match step {
                NetStep::Expect(expected)
                    if self.written.len() - self.consumed >= expected.len() =>
                {
                    if self.written[self.consumed..][..expected.len()] != *expected {
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
    next_connection: Vec<usize>,
    streams: BTreeMap<u64, Connection>,
}
impl ScriptNet {
    pub fn new(peers: Vec<NetPeer>) -> Result<Self, AdapterError> {
        let mut endpoints = BTreeMap::new();
        for (index, p) in peers.iter().enumerate() {
            if let Some(first) = endpoints.insert((&p.address, p.port), index) {
                return Err(AdapterError::Unsupported(format!(
                    "duplicate address/port at entries {first} and {index}"
                )));
            }
            if !canonical_address(&p.address)
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
            streams: BTreeMap::new(),
        })
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
                self.streams.insert(
                    id,
                    Connection {
                        peer: i,
                        tls: false,
                        steps: steps.clone(),
                        step: 0,
                        offset: 0,
                        written: vec![],
                        consumed: 0,
                        reset: false,
                        read_timeout: false,
                        read_closed: false,
                        write_closed: false,
                    },
                );
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
impl NetProvider for ScriptNet {
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
        let peer_index = s.peer;
        let peer = &self.peers[peer_index];
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
    fn tls_connect(&mut self, id: u64, server_name: &str, _roots: &RootsDigest) -> Provided<()> {
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
        let peer_index = s.peer;
        let peer = &self.peers[peer_index];
        Ok(match &peer.tls {
            PeerTls::Plain => Err(NetError::Tls(TlsFailure::HandshakeFailure)),
            PeerTls::Certificate(names) if names.iter().any(|n| n == server_name) => {
                self.connection(id)?.tls = true;
                Ok(())
            }
            PeerTls::Certificate(_) => Err(NetError::Tls(TlsFailure::NameMismatch)),
            PeerTls::Fails(failure) => Err(NetError::Tls((*failure).into())),
        })
    }
    fn tls_close(&mut self, id: u64) -> Provided<()> {
        let s = self.connection(id)?;
        Ok(if s.tls { s.check() } else { Ok(()) })
    }
    fn listen(&mut self, _id: u64, _addr: SocketAddr) -> Provided<()> {
        Err(AdapterError::Unsupported(
            "net.listen requires M2 runtime".into(),
        ))
    }
    fn accept(&mut self, _listener: u64, _id: u64) -> Provided<SocketAddr> {
        Err(AdapterError::Unsupported(
            "net.accept requires M2 runtime".into(),
        ))
    }
    fn local_addr(&mut self, _listener: u64) -> Provided<SocketAddr> {
        Err(AdapterError::Unsupported(
            "net.local_addr requires M2 runtime".into(),
        ))
    }
    fn close(&mut self, id: u64) {
        self.streams.remove(&id);
    }
}

/// Cloneable, socket-free network capability model for driver-owned execution
/// oracles. Policy, brands and peer transitions are the same State/ScriptNet
/// path used by native execution; no runtime allocation helper is involved.
#[derive(Clone, Debug, Default)]
pub struct ScriptIo {
    state: crate::io::State,
}
impl ScriptIo {
    pub fn new(test_seed: Option<u64>) -> Self {
        let mut model = Self::default();
        if let Some(seed) = test_seed {
            model.state.handles = crate::handles::Handles::test(seed);
        }
        model
    }
    pub fn call(
        &mut self,
        request: crate::io::Request,
    ) -> Result<crate::io::Completion, AdapterError> {
        use crate::io::{Request, Tree};
        if !crate::net::is_net_request(&request)
            && !matches!(request, Request::Net { .. } | Request::Close { .. })
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
