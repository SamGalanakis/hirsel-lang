//! Cooperative host provider. Socket conventions and poll/SO_ERROR handling follow
//! Rust std's Unix Socket::connect_timeout, read, write and set_nonblocking.
//! Unlike the blocking provider, EAGAIN retains readiness instead of becoming a timeout.
use super::*;
use crate::io::AdapterError;
use crate::readiness::Readiness;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{Receiver, TryRecvError};

/// Separate provider selection leaves native HostNet's blocking contract intact.
pub struct CooperativeNet {
    host: HostNet,
    readiness: Readiness,
    timeouts: BTreeMap<u64, (NetTimeout, NetTimeout)>,
}
impl CooperativeNet {
    /// Blocked operations record their interest in the scheduler's shared readiness.
    pub fn new(readiness: Readiness) -> Self {
        Self {
            host: HostNet::new(),
            readiness,
            timeouts: BTreeMap::new(),
        }
    }
    fn interest(
        &self,
        fd: libc::c_int,
        events: libc::c_short,
        timeout: NetTimeout,
    ) -> Result<(), NetError> {
        let mut pending = self.readiness.pending();
        if pending.interest.is_none() {
            pending.deadline = duration(timeout).and_then(|d| Instant::now().checked_add(d));
        }
        pending.interest = Some((fd, events));
        if pending.deadline.is_some_and(|d| Instant::now() >= d) {
            Err(NetError::TimedOut)
        } else {
            Ok(())
        }
    }
    fn timeout(&self, stream: u64, write: bool) -> NetTimeout {
        self.timeouts
            .get(&stream)
            .map_or(NetTimeout::Unbounded, |t| if write { t.1 } else { t.0 })
    }
    fn blocked<T>(
        &self,
        fd: libc::c_int,
        events: libc::c_short,
        timeout: NetTimeout,
    ) -> Provided<T> {
        match self.interest(fd, events, timeout) {
            Ok(()) => Err(AdapterError::Blocked),
            Err(e) => Ok(Err(e)),
        }
    }
    fn resolve_async(&mut self, host: &str, port: u16) -> Provided<Vec<SocketAddr>> {
        if let Ok(ip) = host.parse() {
            return Ok(Ok(vec![SocketAddr { ip, port }]));
        }
        let mut pending = self.readiness.pending();
        if pending.resolver.is_none() {
            let (wake, mut notify) = match UnixStream::pair() {
                Ok(pair) => pair,
                Err(_) => return Ok(Err(NetError::Fault(1))),
            };
            let (send, recv) = std::sync::mpsc::channel();
            let host = host.to_owned();
            match std::thread::Builder::new()
                .name("hirsel-resolver".into())
                .spawn(move || {
                    let _ = send.send(resolve(&host, port));
                    let _ = notify.write_all(&[1]);
                }) {
                Ok(_) => pending.committed = true,
                Err(_) => return Ok(Err(NetError::Fault(1))),
            }
            pending.interest = Some((wake.as_raw_fd(), libc::POLLIN));
            pending.resolver = Some((recv, wake));
        }
        match pending.resolver.as_ref().unwrap().0.try_recv() {
            Ok(result) => {
                pending.resolver = None;
                pending.interest = None;
                Ok(result)
            }
            Err(TryRecvError::Empty) => Err(AdapterError::Blocked),
            Err(TryRecvError::Disconnected) => Ok(Err(NetError::ResolverFailure)),
        }
    }
    fn connect_addresses(&mut self, id: u64) -> Provided<()> {
        loop {
            let mut pending = self.readiness.pending();
            if pending.deadline.is_some_and(|d| Instant::now() >= d) {
                return Ok(Err(NetError::TimedOut));
            }
            if let Some(socket) = pending.connecting.take() {
                let fd = socket.as_raw_fd();
                let mut row = libc::pollfd {
                    fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                // SAFETY: socket owns fd and row is valid.
                let n = unsafe { libc::poll(&mut row, 1, 0) };
                if n == 0 || n == -1 && errno() == libc::EINTR {
                    pending.connecting = Some(socket);
                    return Err(AdapterError::Blocked);
                }
                let status = if n == -1 {
                    Err(error(errno()))
                } else {
                    take_error(fd)
                        .and_then(|code| if code == 0 { Ok(()) } else { Err(error(code)) })
                };
                match status {
                    Ok(()) => {
                        drop(pending);
                        self.host.sockets.insert(id, socket);
                        return Ok(Ok(()));
                    }
                    Err(e) => pending.last_error = Some(e),
                }
            }
            let Some(address) = pending.addresses.as_mut().and_then(VecDeque::pop_front) else {
                return Ok(Err(pending.last_error.unwrap_or(NetError::UnknownHost)));
            };
            let socket = match new_socket(address.ip) {
                Ok(s) => s,
                Err(e) => {
                    pending.last_error = Some(e);
                    continue;
                }
            };
            let fd = socket.as_raw_fd();
            if let Err(e) = set_nonblocking(fd, true) {
                return Ok(Err(e));
            }
            let (storage, len) = to_c(address);
            // SAFETY: storage contains a sockaddr of len bytes.
            let n = unsafe { libc::connect(fd, (&raw const storage).cast(), len) };
            // Once connect may have sent a packet, cancellation cannot promise
            // no effect (core network contract), even while readiness is pending.
            pending.committed = true;
            if n == 0 {
                drop(pending);
                self.host.sockets.insert(id, socket);
                return Ok(Ok(()));
            }
            let code = errno();
            if code == libc::EINPROGRESS || code == libc::EINTR {
                pending.connecting = Some(socket);
                pending.interest = Some((fd, libc::POLLOUT));
                return Err(AdapterError::Blocked);
            }
            pending.last_error = Some(error(code));
        }
    }
    fn send(&mut self, stream: u64, bytes: &[u8]) -> Provided<u64> {
        let fd = match self.host.socket(stream) {
            Ok(fd) => fd,
            Err(e) => return Ok(Err(e)),
        };
        if let Some(conn) = self.host.tls.get_mut(&stream) {
            // rustls accepts plaintext into its bounded buffer; flushing below
            // retains the connection's encrypted output across readiness waits.
            return Ok(conn
                .writer()
                .write(bytes)
                .map(|n| n as u64)
                .map_err(crate::net_tls::error));
        }
        loop {
            // SAFETY: bytes is readable and fd is a live nonblocking socket.
            match checked(unsafe {
                libc::send(fd, bytes.as_ptr().cast(), bytes.len(), libc::MSG_NOSIGNAL)
            }) {
                Ok(n) => return Ok(Ok(n as u64)),
                Err(libc::EINTR) => {}
                Err(libc::EAGAIN) => {
                    return self.blocked(fd, libc::POLLOUT, self.timeout(stream, true));
                }
                Err(e) => return Ok(Err(error(e))),
            }
        }
    }
    fn flush_tls(&mut self, stream: u64) -> Provided<()> {
        let fd = match self.host.socket(stream) {
            Ok(fd) => fd,
            Err(e) => return Ok(Err(e)),
        };
        if let Some(conn) = self.host.tls.get_mut(&stream) {
            while conn.wants_write() {
                match conn.write_tls(&mut crate::net_tls::Transport(fd)) {
                    Ok(0) => return Ok(Err(NetError::BrokenPipe)),
                    Ok(_) => self.readiness.pending().committed = true,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        return self.blocked(fd, libc::POLLOUT, self.timeout(stream, true));
                    }
                    Err(e) => return Ok(Err(crate::net_tls::error(e))),
                }
            }
        }
        Ok(Ok(()))
    }
}
impl NetProvider for CooperativeNet {
    fn udp_bind(&mut self, id: u64, addr: SocketAddr) -> Provided<()> {
        match self.host.udp_bind(id, addr)? {
            Ok(()) => match set_nonblocking(self.host.socket(id).unwrap(), true) {
                Ok(()) => Ok(Ok(())),
                Err(e) => {
                    self.host.close(id);
                    Ok(Err(e))
                }
            },
            Err(e) => Ok(Err(e)),
        }
    }
    fn udp_send(&mut self, socket: u64, bytes: &[u8], addr: SocketAddr) -> Provided<u64> {
        match self.host.udp_send(socket, bytes, addr)? {
            Err(NetError::TimedOut) => self.blocked(
                self.host.socket(socket).unwrap(),
                libc::POLLOUT,
                NetTimeout::Unbounded,
            ),
            result => Ok(result),
        }
    }
    fn udp_recv(&mut self, socket: u64) -> Provided<(Vec<u8>, SocketAddr)> {
        match self.host.udp_recv(socket)? {
            Err(NetError::TimedOut) => self.blocked(
                self.host.socket(socket).unwrap(),
                libc::POLLIN,
                NetTimeout::Unbounded,
            ),
            result => Ok(result),
        }
    }

    fn tls_roots(&mut self, pem: &[u8]) {
        self.host.tls_roots(pem);
    }
    fn resolve(&mut self, host: &str, port: u16) -> Provided<Vec<SocketAddr>> {
        self.resolve_async(host, port)
    }
    fn connect(&mut self, id: u64, host: &str, port: u16, timeout: NetTimeout) -> Provided<()> {
        if self.readiness.pending().addresses.is_none() {
            // The relative timeout includes name resolution.
            if self.readiness.pending().deadline.is_none() {
                self.readiness.pending().deadline =
                    duration(timeout).and_then(|d| Instant::now().checked_add(d));
            }
            if self
                .readiness
                .pending()
                .deadline
                .is_some_and(|d| Instant::now() >= d)
            {
                return Ok(Err(NetError::TimedOut));
            }
            let addresses = match self.resolve_async(host, port)? {
                Ok(a) => a,
                Err(e) => return Ok(Err(e)),
            };
            self.readiness.pending().addresses = Some(addresses.into());
        }
        self.connect_addresses(id)
    }
    fn connect_addr(&mut self, id: u64, addr: SocketAddr, timeout: NetTimeout) -> Provided<()> {
        if self.readiness.pending().addresses.is_none() {
            self.readiness.pending().addresses = Some(VecDeque::from([addr]));
            self.readiness.pending().deadline =
                duration(timeout).and_then(|d| Instant::now().checked_add(d));
        }
        self.connect_addresses(id)
    }
    fn read(&mut self, stream: u64, max: u64) -> Provided<Vec<u8>> {
        let fd = match self.host.socket(stream) {
            Ok(fd) => fd,
            Err(e) => return Ok(Err(e)),
        };
        let mut buffer = vec![0; max.min(READ_CHUNK) as usize];
        if self.host.tls.contains_key(&stream) {
            match self.flush_tls(stream)? {
                Ok(()) => {}
                Err(e) => return Ok(Err(e)),
            }
            loop {
                let conn = self.host.tls.get_mut(&stream).unwrap();
                match conn.reader().read(&mut buffer) {
                    Ok(n) => {
                        buffer.truncate(n);
                        return Ok(Ok(buffer));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Ok(Err(crate::net_tls::error(e))),
                }
                match conn.read_tls(&mut crate::net_tls::Transport(fd)) {
                    Ok(_) => {
                        if let Err(e) = conn.process_new_packets() {
                            return Ok(Err(NetError::Tls(crate::net_tls::failure(&e))));
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        return self.blocked(fd, libc::POLLIN, self.timeout(stream, false));
                    }
                    Err(e) => return Ok(Err(crate::net_tls::error(e))),
                }
            }
        }
        loop {
            // SAFETY: buffer is writable and the socket is nonblocking.
            match checked(unsafe { libc::recv(fd, buffer.as_mut_ptr().cast(), buffer.len(), 0) }) {
                Ok(n) => {
                    buffer.truncate(n);
                    return Ok(Ok(buffer));
                }
                Err(libc::EINTR) => {}
                Err(libc::EAGAIN) => {
                    return self.blocked(fd, libc::POLLIN, self.timeout(stream, false));
                }
                Err(e) => return Ok(Err(error(e))),
            }
        }
    }
    fn write(&mut self, stream: u64, bytes: &[u8]) -> Provided<u64> {
        if !self.host.tls.contains_key(&stream) {
            return self.send(stream, bytes);
        }
        if self.readiness.pending().offset == 0 {
            match self.flush_tls(stream)? {
                Ok(()) => {}
                Err(e) => return Ok(Err(e)),
            }
            match self.send(stream, bytes)? {
                Ok(n) => {
                    let mut pending = self.readiness.pending();
                    pending.offset = n as usize;
                    pending.committed = n != 0;
                }
                Err(e) => return Ok(Err(e)),
            }
        }
        match self.flush_tls(stream)? {
            Ok(()) => Ok(Ok(self.readiness.pending().offset as u64)),
            Err(e) => Ok(Err(e)),
        }
    }
    fn write_all(&mut self, stream: u64, bytes: &[u8]) -> Provided<()> {
        loop {
            let offset = self.readiness.pending().offset;
            if offset == bytes.len() {
                return self.flush_tls(stream);
            }
            match self.flush_tls(stream)? {
                Ok(()) => {}
                Err(e) => return Ok(Err(e)),
            }
            match self.send(stream, &bytes[offset..])? {
                Ok(0) => return Ok(Err(NetError::BrokenPipe)),
                Ok(n) => {
                    let mut pending = self.readiness.pending();
                    pending.offset += n as usize;
                    pending.committed = true;
                }
                Err(e) => return Ok(Err(e)),
            }
        }
    }
    fn shutdown(&mut self, stream: u64, how: ShutdownHow) -> Provided<()> {
        self.host.shutdown(stream, how)
    }
    fn set_timeout(
        &mut self,
        stream: u64,
        which: TimeoutKind,
        timeout: NetTimeout,
    ) -> Provided<()> {
        let entry = self
            .timeouts
            .entry(stream)
            .or_insert((NetTimeout::Unbounded, NetTimeout::Unbounded));
        match which {
            TimeoutKind::Read => entry.0 = timeout,
            TimeoutKind::Write => entry.1 = timeout,
        }
        Ok(Ok(()))
    }
    fn peer_addr(&mut self, stream: u64) -> Provided<SocketAddr> {
        self.host.peer_addr(stream)
    }
    fn tls_connect(&mut self, stream: u64, name: &str, roots: &RootsDigest) -> Provided<()> {
        let fd = match self.host.socket(stream) {
            Ok(fd) => fd,
            Err(e) => return Ok(Err(e)),
        };
        if let std::collections::btree_map::Entry::Vacant(entry) = self.host.tls.entry(stream) {
            let conn = match crate::net_tls::client(name, roots, self.host.pem.take()) {
                Ok(c) => c,
                Err(e) => return Ok(Err(e)),
            };
            entry.insert(conn);
        }
        loop {
            match self.flush_tls(stream)? {
                Ok(()) => {}
                Err(e) => return Ok(Err(e)),
            }
            let conn = self.host.tls.get_mut(&stream).unwrap();
            if !conn.is_handshaking() {
                return Ok(Ok(()));
            }
            match conn.read_tls(&mut crate::net_tls::Transport(fd)) {
                Ok(0) => return Ok(Err(NetError::Tls(crate::net::TlsFailure::Truncated))),
                Ok(_) => {
                    if let Err(e) = conn.process_new_packets() {
                        return Ok(Err(NetError::Tls(crate::net_tls::failure(&e))));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return self.blocked(fd, libc::POLLIN, self.timeout(stream, false));
                }
                Err(e) => return Ok(Err(crate::net_tls::error(e))),
            }
        }
    }
    fn tls_close(&mut self, stream: u64) -> Provided<()> {
        if let Some(conn) = self.host.tls.get_mut(&stream) {
            if self.readiness.pending().offset == 0 {
                conn.send_close_notify();
                self.readiness.pending().offset = 1;
            }
        }
        self.flush_tls(stream)
    }
    fn listen(&mut self, id: u64, addr: SocketAddr) -> Provided<()> {
        match self.host.listen(id, addr)? {
            Ok(()) => {}
            Err(e) => return Ok(Err(e)),
        }
        if let Err(e) = set_nonblocking(self.host.socket(id).unwrap(), true) {
            self.host.close(id);
            return Ok(Err(e));
        }
        Ok(Ok(()))
    }
    fn accept(&mut self, listener: u64, id: u64) -> Provided<SocketAddr> {
        // The listener is nonblocking. accept4 itself never waits.
        match self.host.accept(listener, id)? {
            Ok(addr) => match set_nonblocking(self.host.socket(id).unwrap(), true) {
                Ok(()) => Ok(Ok(addr)),
                Err(e) => {
                    self.host.close(id);
                    Ok(Err(e))
                }
            },
            Err(NetError::TimedOut) => self.blocked(
                self.host.socket(listener).unwrap(),
                libc::POLLIN,
                NetTimeout::Unbounded,
            ),
            Err(e) => Ok(Err(e)),
        }
    }
    fn local_addr(&mut self, listener: u64) -> Provided<SocketAddr> {
        self.host.local_addr(listener)
    }
    fn close(&mut self, handle: u64) {
        self.timeouts.remove(&handle);
        self.host.close(handle);
    }
}
