//! Cooperative host provider. Socket conventions and poll/SO_ERROR handling follow
//! Rust std's Unix Socket::connect_timeout, read, write and set_nonblocking.
//! Unlike the blocking provider, EAGAIN retains readiness instead of becoming a timeout.
//! A Unix connect facing a full backlog keeps its nonblocking socket and connects again
//! after a short timer wait, since Linux signals no room in a listener's backlog.
//!
//! Borrowed reads follow Rust a7c37f9f library/std/src/sys/net/connection/socket/
//! unix.rs:285-303,320-322 (recv into BorrowedCursor, advance only its written
//! prefix). The syscall-first path keeps Tokio ce685ee6 tokio/src/runtime/io/
//! registration.rs:225-247's retry-on-WouldBlock structure, using Hirsel's
//! stackful parked call in place of an async future. Recording retains owned
//! buffers because its tape must retain the delivered bytes.
use super::*;
use crate::io::AdapterError;
use crate::net_tls::Stall;
use crate::readiness::Readiness;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{Receiver, TryRecvError};

/// How long a Unix connect facing a full backlog waits before connecting again.
const UNIX_CONNECT_RETRY: Duration = Duration::from_millis(5);

/// Separate provider selection leaves native HostNet's blocking contract intact.
pub struct CooperativeNet {
    host: HostNet,
    readiness: Readiness,
    timeouts: IdMap<(NetTimeout, NetTimeout)>,
}
impl CooperativeNet {
    /// Blocked operations record their interest in the scheduler's shared readiness.
    pub fn new(readiness: Readiness) -> Self {
        Self {
            host: HostNet::new(),
            readiness,
            timeouts: IdMap::default(),
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
            return Ok(conn.write_plaintext(bytes).map(|n| n as u64));
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
    // Nonblocking TLS reference: rustls 0.23.46, f04804f, examples/src/bin/
    // tlsclient-mio.rs:88-124 (read_tls/process_new_packets), :147-185
    // (write_tls and wants_read/wants_write readiness). Hirsel parks the current
    // operation on WouldBlock rather than reregistering a mio client; readiness,
    // deadlines and committed writes belong to its replayable provider boundary.
    /// Start a handshake on its first issue, then drive it across readiness waits.
    fn handshake(
        &mut self,
        stream: u64,
        start: impl FnOnce() -> Result<Box<dyn crate::net_tls::TlsSession>, NetError>,
    ) -> Provided<Option<String>> {
        let fd = match self.host.socket(stream) {
            Ok(fd) => fd,
            Err(e) => return Ok(Err(e)),
        };
        if let std::collections::hash_map::Entry::Vacant(entry) = self.host.tls.entry(stream) {
            match start() {
                Ok(conn) => entry.insert(conn),
                Err(e) => return Ok(Err(e)),
            };
        }
        loop {
            match self.flush_tls(stream)? {
                Ok(()) => {}
                Err(e) => return Ok(Err(e)),
            }
            let conn = self.host.tls.get_mut(&stream).unwrap();
            if !conn.is_handshaking() {
                return Ok(Ok(conn.protocol()));
            }
            match conn.read_tls(fd) {
                Ok(0) => return Ok(Err(NetError::Tls(crate::net::TlsFailure::Truncated))),
                Ok(_) => {
                    if let Err(failure) = conn.process_new_packets() {
                        // Best effort, as rustls complete_io does: tell the peer why, so a
                        // refused client certificate reaches the client as its alert.
                        let _ = conn.write_tls(fd);
                        return Ok(Err(NetError::Tls(failure)));
                    }
                }
                Err(Stall::WouldBlock) => {
                    return self.blocked(fd, libc::POLLIN, self.timeout(stream, false));
                }
                Err(Stall::Failed(e)) => return Ok(Err(e)),
            }
        }
    }
    fn flush_tls(&mut self, stream: u64) -> Provided<()> {
        if let Some(conn) = self.host.tls.get_mut(&stream) {
            let fd = match self.host.sockets.get(&stream) {
                Some(fd) => fd.as_raw_fd(),
                None => return Ok(Err(NetError::NotConnected)),
            };
            while conn.wants_write() {
                match conn.write_tls(fd) {
                    Ok(0) => return Ok(Err(NetError::BrokenPipe)),
                    Ok(_) => self.readiness.pending().committed = true,
                    Err(Stall::WouldBlock) => {
                        return self.blocked(fd, libc::POLLOUT, self.timeout(stream, true));
                    }
                    Err(Stall::Failed(e)) => return Ok(Err(e)),
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
                match conn.read_plaintext(&mut buffer) {
                    Ok(n) => {
                        buffer.truncate(n);
                        return Ok(Ok(buffer));
                    }
                    Err(Stall::WouldBlock) => {}
                    Err(Stall::Failed(e)) => return Ok(Err(e)),
                }
                match conn.read_tls(fd) {
                    Ok(_) => {
                        if let Err(failure) = conn.process_new_packets() {
                            return Ok(Err(NetError::Tls(failure)));
                        }
                    }
                    Err(Stall::WouldBlock) => {
                        return self.blocked(fd, libc::POLLIN, self.timeout(stream, false));
                    }
                    Err(Stall::Failed(e)) => return Ok(Err(e)),
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
    fn read_uninit(
        &mut self,
        stream: u64,
        buffer: &mut [std::mem::MaybeUninit<u8>],
    ) -> Provided<usize> {
        if self.host.tls.contains_key(&stream) {
            return self.read(stream, buffer.len() as u64).map(|result| {
                result.map(|bytes| {
                    for (slot, byte) in buffer.iter_mut().zip(&bytes) {
                        slot.write(*byte);
                    }
                    bytes.len()
                })
            });
        }
        let fd = match self.host.socket(stream) {
            Ok(fd) => fd,
            Err(e) => return Ok(Err(e)),
        };
        let capacity = buffer.len().min(READ_CHUNK as usize);
        loop {
            // SAFETY: recv initializes only its returned prefix in writable spare capacity.
            match checked(unsafe { libc::recv(fd, buffer.as_mut_ptr().cast(), capacity, 0) }) {
                Ok(n) => return Ok(Ok(n)),
                Err(libc::EINTR) => {}
                Err(libc::EAGAIN) => {
                    return self.blocked(fd, libc::POLLIN, self.timeout(stream, false));
                }
                Err(code) => return Ok(Err(error(code))),
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
        let mut offset = self.readiness.offset();
        loop {
            match self.flush_tls(stream) {
                Ok(Ok(())) => {}
                result => {
                    let mut pending = self.readiness.pending();
                    pending.offset = offset;
                    pending.committed |= offset != 0;
                    return result;
                }
            }
            if offset == bytes.len() {
                return Ok(Ok(()));
            }
            match self.send(stream, &bytes[offset..]) {
                Ok(Ok(0)) => return Ok(Err(NetError::BrokenPipe)),
                Ok(Ok(n)) => offset += n as usize,
                result => {
                    let mut pending = self.readiness.pending();
                    pending.offset = offset;
                    pending.committed |= offset != 0;
                    return result.map(|value| value.map(|_| ()));
                }
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
    fn tls_connect(
        &mut self,
        tls: &dyn crate::net_tls::TlsEngine,
        stream: u64,
        name: &str,
        roots: &RootsDigest,
        identity: Option<&std::sync::Arc<hirsel_run_schema::tls::Identity>>,
        alpn: &[String],
    ) -> Provided<Option<String>> {
        let pem = self.host.pem.take();
        self.handshake(stream, || tls.connect(name, roots, pem, identity, alpn))
    }
    fn tls_accept(
        &mut self,
        tls: &dyn crate::net_tls::TlsEngine,
        stream: u64,
        server: &hirsel_run_schema::tls::Server,
    ) -> Provided<Option<String>> {
        self.handshake(stream, || tls.accept(server))
    }
    fn tls_peer(&mut self, stream: u64) -> Provided<Option<Vec<u8>>> {
        self.host.tls_peer(stream)
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
    fn unix_connect(&mut self, id: u64, path: &str) -> Provided<()> {
        let address = match unix_address(path) {
            Ok(address) => address,
            Err(e) => return Ok(Err(e)),
        };
        let mut pending = self.readiness.pending();
        let socket = match pending.connecting.take() {
            Some(socket) => socket,
            None => match unix_socket()
                .and_then(|socket| set_nonblocking(socket.as_raw_fd(), true).map(|()| socket))
            {
                Ok(socket) => socket,
                Err(e) => return Ok(Err(e)),
            },
        };
        match unix_connect_on(socket.as_raw_fd(), &address) {
            Ok(true) => {
                drop(pending);
                self.host.sockets.insert(id, socket);
                Ok(Ok(()))
            }
            // The backlog is full and nothing was queued, so the wait stays cancellable.
            // Nothing signals room in a backlog: the wait is a timer, then a fresh attempt.
            Ok(false) => {
                pending.connecting = Some(socket);
                pending.interest = None;
                pending.deadline = Some(Instant::now() + UNIX_CONNECT_RETRY);
                Err(AdapterError::Blocked)
            }
            Err(e) => Ok(Err(e)),
        }
    }
    fn unix_listen(&mut self, id: u64, path: &str, remove_stale: bool) -> Provided<()> {
        match self.host.unix_listen(id, path, remove_stale)? {
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

#[cfg(test)]
mod borrowed_buffer_tests {
    use super::*;
    use std::mem::MaybeUninit;

    fn pair() -> (CooperativeNet, UnixStream, Readiness) {
        let (socket, peer) = UnixStream::pair().unwrap();
        socket.set_nonblocking(true).unwrap();
        peer.set_nonblocking(true).unwrap();
        let size: libc::c_int = 4096;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&raw const size).cast(),
                    size_of_val(&size) as _,
                )
            },
            0
        );
        let readiness = Readiness::default();
        let mut provider = CooperativeNet::new(readiness.clone());
        provider.host.sockets.insert(1, socket.into());
        (provider, peer, readiness)
    }

    #[test]
    fn uninitialized_read_survives_would_block_without_zeroing_the_tail() {
        let (mut provider, mut peer, readiness) = pair();
        readiness.enter(7);
        let mut bytes = [MaybeUninit::new(0xa5); 32];
        assert_eq!(
            provider.read_uninit(1, &mut bytes),
            Err(AdapterError::Blocked)
        );
        peer.write_all(b"abc").unwrap();
        readiness.parked(7);
        assert_eq!(readiness.ready().unwrap(), [7]);
        assert_eq!(provider.read_uninit(1, &mut bytes), Ok(Ok(3)));
        readiness.finish(7);
        let bytes = bytes.map(|v| unsafe { v.assume_init() });
        assert_eq!(&bytes[..3], b"abc");
        assert_eq!(&bytes[3..], &[0xa5; 29]);
    }

    #[test]
    fn partial_borrowed_write_retries_only_the_unwritten_suffix() {
        let (mut provider, mut peer, readiness) = pair();
        readiness.enter(8);
        let bytes: Vec<u8> = (0..1024 * 1024).map(|n| (n % 251) as u8).collect();
        assert_eq!(provider.write_all(1, &bytes), Err(AdapterError::Blocked));
        assert!(
            !readiness.cancellable(8),
            "a partial send committed an effect"
        );
        let mut received = Vec::new();
        let mut buffer = [0; 16384];
        loop {
            loop {
                match peer.read(&mut buffer) {
                    Ok(0) => panic!("early EOF"),
                    Ok(n) => received.extend_from_slice(&buffer[..n]),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => panic!("{error}"),
                }
            }
            match provider.write_all(1, &bytes) {
                Err(AdapterError::Blocked) => continue,
                Ok(Ok(())) => break,
                result => panic!("{result:?}"),
            }
        }
        while received.len() < bytes.len() {
            let n = peer.read(&mut buffer).unwrap();
            assert_ne!(n, 0);
            received.extend_from_slice(&buffer[..n]);
        }
        assert_eq!(received, bytes);
        readiness.finish(8);
    }
}
