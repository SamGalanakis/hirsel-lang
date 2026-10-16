//! The host network provider (`net.stream@1`): TCP sockets and name resolution
//! through libc. Ported from Rust std `library/std/src/sys/net/connection/socket/unix.rs`
//! (`connect_timeout`, `set_timeout`, `read`, `write`, `shutdown`, `accept`; f6c1107f) and
//! `library/std/src/sys/pal/unix/mod.rs` (`decode_error_kind`), and Zig
//! `lib/std/Io/Threaded.zig` (getaddrinfo hints and EAI mapping; bff490a2). Policy already
//! ran: every request reaching this provider was granted.
//!
//! Deviations: connect over several resolved addresses tries them one at a time in resolver
//! order with one deadline for the whole call (Rust `each_addr`, not Zig's concurrent
//! `connectMany`); every socket is created close-on-exec and writes pass
//! `MSG_NOSIGNAL` as Rust std does on Linux; reads deliver at most `READ_CHUNK` bytes.
use crate::net::{
    NetError, NetProvider, NetTimeout, Provided, RootsDigest, ShutdownHow, SocketAddr, TimeoutKind,
};
use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

/// The largest single read; a read never waits to fill its limit (§22 Network).
pub const READ_CHUNK: u64 = 64 * 1024;
/// Zig `ListenOptions.kernel_backlog`.
const BACKLOG: libc::c_int = 128;

/// Open sockets keyed by their tape-local handle.
#[derive(Debug, Default)]
pub struct HostNet {
    sockets: BTreeMap<u64, OwnedFd>,
    tls: BTreeMap<u64, rustls::ClientConnection>,
    pem: Option<Vec<u8>>,
}
impl HostNet {
    pub fn new() -> Self {
        Self::default()
    }
    fn socket(&self, id: u64) -> Result<libc::c_int, NetError> {
        self.sockets
            .get(&id)
            .map(|fd| fd.as_raw_fd())
            .ok_or(NetError::NotConnected)
    }
}

fn errno() -> libc::c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Rust `decode_error_kind`, restricted to the kinds sockets produce. `EAGAIN` is a socket
/// timeout here: every stream blocks, so it arises only from SO_RCVTIMEO/SO_SNDTIMEO.
pub fn error(code: libc::c_int) -> NetError {
    match code {
        libc::ECONNREFUSED => NetError::ConnectionRefused,
        libc::ECONNRESET => NetError::ConnectionReset,
        libc::ECONNABORTED => NetError::ConnectionAborted,
        libc::ENOTCONN => NetError::NotConnected,
        libc::EADDRINUSE => NetError::AddrInUse,
        libc::EADDRNOTAVAIL => NetError::AddrNotAvailable,
        libc::EHOSTUNREACH => NetError::HostUnreachable,
        libc::ENETUNREACH => NetError::NetworkUnreachable,
        libc::ENETDOWN => NetError::NetworkDown,
        libc::EPIPE => NetError::BrokenPipe,
        libc::ETIMEDOUT | libc::EAGAIN | libc::EINPROGRESS => NetError::TimedOut,
        libc::EINVAL => NetError::InvalidInput,
        // A capability input, never an errno contract (§22).
        _ => NetError::Fault(1),
    }
}

fn duration(timeout: NetTimeout) -> Option<Duration> {
    match timeout {
        NetTimeout::Unbounded => None,
        NetTimeout::Nanos(n) => Some(Duration::from_nanos(n)),
    }
}

/// Rust `socket_addr_to_c`.
fn to_c(addr: SocketAddr) -> (libc::sockaddr_storage, libc::socklen_t) {
    // SAFETY: an all-zero sockaddr_storage is a valid unspecified address.
    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let len = match addr.ip {
        std::net::IpAddr::V4(ip) => {
            let sin = libc::sockaddr_in {
                sin_family: libc::AF_INET as libc::sa_family_t,
                sin_port: addr.port.to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes(ip.octets()),
                },
                sin_zero: [0; 8],
            };
            // SAFETY: sockaddr_storage is large and aligned enough for any sockaddr.
            unsafe { std::ptr::write((&raw mut storage).cast(), sin) };
            std::mem::size_of::<libc::sockaddr_in>()
        }
        std::net::IpAddr::V6(ip) => {
            let sin6 = libc::sockaddr_in6 {
                sin6_family: libc::AF_INET6 as libc::sa_family_t,
                sin6_port: addr.port.to_be(),
                sin6_flowinfo: 0,
                sin6_addr: libc::in6_addr {
                    s6_addr: ip.octets(),
                },
                sin6_scope_id: 0,
            };
            // SAFETY: as above.
            unsafe { std::ptr::write((&raw mut storage).cast(), sin6) };
            std::mem::size_of::<libc::sockaddr_in6>()
        }
    };
    (storage, len as libc::socklen_t)
}

/// Rust `sockaddr_to_addr`, dropping flowinfo and scope.
fn from_c(storage: &libc::sockaddr_storage) -> Result<SocketAddr, NetError> {
    match libc::c_int::from(storage.ss_family) {
        libc::AF_INET => {
            // SAFETY: the family says this storage holds a sockaddr_in.
            let sin =
                unsafe { &*(storage as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>() };
            Ok(SocketAddr {
                ip: std::net::Ipv4Addr::from(sin.sin_addr.s_addr.to_ne_bytes()).into(),
                port: u16::from_be(sin.sin_port),
            })
        }
        libc::AF_INET6 => {
            // SAFETY: the family says this storage holds a sockaddr_in6.
            let sin6 = unsafe {
                &*(storage as *const libc::sockaddr_storage).cast::<libc::sockaddr_in6>()
            };
            Ok(SocketAddr {
                ip: std::net::Ipv6Addr::from(sin6.sin6_addr.s6_addr).into(),
                port: u16::from_be(sin6.sin6_port),
            })
        }
        _ => Err(NetError::InvalidInput),
    }
}

fn name_of(
    fd: libc::c_int,
    f: unsafe extern "C" fn(libc::c_int, *mut libc::sockaddr, *mut libc::socklen_t) -> libc::c_int,
) -> Result<SocketAddr, NetError> {
    // SAFETY: zeroed storage is valid; f writes at most len bytes into it.
    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    if unsafe { f(fd, (&raw mut storage).cast(), &mut len) } == -1 {
        return Err(error(errno()));
    }
    from_c(&storage)
}

fn new_socket(ip: std::net::IpAddr) -> Result<OwnedFd, NetError> {
    let family = if ip.is_ipv4() {
        libc::AF_INET
    } else {
        libc::AF_INET6
    };
    // SAFETY: plain socket creation; the descriptor is owned immediately.
    let fd = unsafe { libc::socket(family, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd == -1 {
        return Err(error(errno()));
    }
    // SAFETY: fd is a fresh descriptor owned by nobody else.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn set_nonblocking(fd: libc::c_int, nonblocking: bool) -> Result<(), NetError> {
    let mut flag = libc::c_int::from(nonblocking);
    // SAFETY: FIONBIO reads one int.
    if unsafe { libc::ioctl(fd, libc::FIONBIO, &mut flag) } == -1 {
        return Err(error(errno()));
    }
    Ok(())
}

/// Rust `Socket::take_error`.
fn take_error(fd: libc::c_int) -> Result<libc::c_int, NetError> {
    let mut value: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: SO_ERROR writes one int.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            (&raw mut value).cast(),
            &mut len,
        )
    } == -1
    {
        return Err(error(errno()));
    }
    Ok(value)
}

/// Rust `Socket::connect_timeout`, with an unbounded wait for `None` so EINTR never leaves a
/// half-started blocking connect, and a caller-supplied deadline shared across addresses.
fn connect_one(addr: SocketAddr, deadline: Option<Instant>) -> Result<OwnedFd, NetError> {
    let socket = new_socket(addr.ip)?;
    let fd = socket.as_raw_fd();
    set_nonblocking(fd, true)?;
    let (storage, len) = to_c(addr);
    // SAFETY: storage holds a sockaddr of len bytes.
    let started = unsafe { libc::connect(fd, (&raw const storage).cast(), len) };
    if started == -1 {
        let code = errno();
        if code != libc::EINPROGRESS && code != libc::EINTR {
            return Err(error(code));
        }
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        loop {
            let wait = match deadline {
                None => -1,
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(NetError::TimedOut);
                    }
                    let left = deadline - now;
                    let ms = left
                        .as_secs()
                        .saturating_mul(1_000)
                        .saturating_add(u64::from(left.subsec_nanos()) / 1_000_000)
                        .max(1);
                    ms.min(libc::c_int::MAX as u64) as libc::c_int
                }
            };
            // SAFETY: one live pollfd.
            match unsafe { libc::poll(&mut pollfd, 1, wait) } {
                -1 if errno() == libc::EINTR => {}
                -1 => return Err(error(errno())),
                0 => {}
                _ => {
                    // Linux reports POLLOUT|POLLERR|POLLHUP for refused connections.
                    if pollfd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                        let code = take_error(fd)?;
                        return Err(if code == 0 {
                            NetError::ConnectionRefused
                        } else {
                            error(code)
                        });
                    }
                    break;
                }
            }
        }
    }
    set_nonblocking(fd, false)?;
    Ok(socket)
}

fn timeval(timeout: NetTimeout) -> libc::timeval {
    match duration(timeout) {
        None => libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        Some(d) => {
            let mut tv = libc::timeval {
                tv_sec: d.as_secs().min(libc::time_t::MAX as u64) as libc::time_t,
                tv_usec: d.subsec_micros() as libc::suseconds_t,
            };
            if tv.tv_sec == 0 && tv.tv_usec == 0 {
                tv.tv_usec = 1;
            }
            tv
        }
    }
}

fn checked(result: libc::ssize_t) -> Result<usize, libc::c_int> {
    if result == -1 {
        Err(errno())
    } else {
        Ok(result as usize)
    }
}

/// Zig `Threaded.zig` netLookup over libc `getaddrinfo`: numeric service, stream sockets,
/// TCP, any family; results in resolver order.
pub fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, NetError> {
    let name = std::ffi::CString::new(host).map_err(|_| NetError::InvalidInput)?;
    let service = std::ffi::CString::new(port.to_string()).expect("decimal port");
    // SAFETY: zeroed addrinfo is a valid empty hints record.
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_flags = libc::AI_NUMERICSERV;
    hints.ai_family = libc::AF_UNSPEC;
    hints.ai_socktype = libc::SOCK_STREAM;
    hints.ai_protocol = libc::IPPROTO_TCP;
    let mut list: *mut libc::addrinfo = std::ptr::null_mut();
    loop {
        // SAFETY: valid C strings and hints; list receives an owned result.
        let code = unsafe { libc::getaddrinfo(name.as_ptr(), service.as_ptr(), &hints, &mut list) };
        match code {
            0 => break,
            libc::EAI_SYSTEM if errno() == libc::EINTR => continue,
            libc::EAI_NONAME | libc::EAI_NODATA => return Err(NetError::UnknownHost),
            libc::EAI_AGAIN | libc::EAI_FAIL | libc::EAI_FAMILY => {
                return Err(NetError::ResolverFailure);
            }
            _ => return Err(NetError::Fault(1)),
        }
    }
    let mut addresses = vec![];
    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: cursor walks the list getaddrinfo returned until freeaddrinfo.
        let info = unsafe { &*cursor };
        if !info.ai_addr.is_null() {
            // SAFETY: ai_addr holds ai_addrlen bytes of a sockaddr.
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let len = (info.ai_addrlen as usize).min(std::mem::size_of::<libc::sockaddr_storage>());
            unsafe {
                std::ptr::copy_nonoverlapping(
                    info.ai_addr.cast::<u8>(),
                    (&raw mut storage).cast::<u8>(),
                    len,
                )
            };
            if let Ok(address) = from_c(&storage) {
                addresses.push(address);
            }
        }
        cursor = info.ai_next;
    }
    // SAFETY: list came from getaddrinfo and is freed once.
    unsafe { libc::freeaddrinfo(list) };
    Ok(addresses)
}

impl HostNet {
    fn connected(
        &mut self,
        id: u64,
        addresses: &[SocketAddr],
        timeout: NetTimeout,
    ) -> Result<(), NetError> {
        let deadline = duration(timeout).map(|d| Instant::now() + d);
        // Rust `each_addr`: the first success, or the last error.
        let mut last = NetError::InvalidInput;
        for address in addresses {
            match connect_one(*address, deadline) {
                Ok(socket) => {
                    assert!(self.sockets.insert(id, socket).is_none());
                    return Ok(());
                }
                Err(NetError::TimedOut) if deadline.is_some_and(|d| Instant::now() >= d) => {
                    return Err(NetError::TimedOut);
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }
    fn set_option(
        &self,
        stream: u64,
        option: libc::c_int,
        tv: libc::timeval,
    ) -> Result<(), NetError> {
        let fd = self.socket(stream)?;
        // SAFETY: the option reads one timeval.
        if unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                (&raw const tv).cast(),
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            )
        } == -1
        {
            return Err(error(errno()));
        }
        Ok(())
    }
    fn send(&mut self, stream: u64, bytes: &[u8]) -> Result<u64, NetError> {
        let fd = self.socket(stream)?;
        if let Some(conn) = self.tls.get_mut(&stream) {
            return crate::net_tls::write(conn, fd, bytes);
        }
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            // SAFETY: borrowed readable bytes.
            match checked(unsafe {
                libc::send(fd, bytes.as_ptr().cast(), bytes.len(), libc::MSG_NOSIGNAL)
            }) {
                Ok(n) => return Ok(n as u64),
                Err(libc::EINTR) => {}
                Err(code) => return Err(error(code)),
            }
        }
    }
}

impl NetProvider for HostNet {
    fn tls_roots(&mut self, pem: &[u8]) {
        self.pem = Some(pem.to_vec());
    }
    fn resolve(&mut self, host: &str, port: u16) -> Provided<Vec<SocketAddr>> {
        Ok(resolve(host, port))
    }
    fn connect(&mut self, id: u64, host: &str, port: u16, timeout: NetTimeout) -> Provided<()> {
        let started = Instant::now();
        Ok(resolve(host, port).and_then(|addresses| {
            // The timeout bounds the whole call, resolution included (§22 Network).
            let timeout = match duration(timeout) {
                None => NetTimeout::Unbounded,
                Some(d) => match d.checked_sub(started.elapsed()) {
                    Some(left) if !left.is_zero() => NetTimeout::Nanos(left.as_nanos() as u64),
                    _ => return Err(NetError::TimedOut),
                },
            };
            if addresses.is_empty() {
                return Err(NetError::UnknownHost);
            }
            self.connected(id, &addresses, timeout)
        }))
    }
    fn connect_addr(&mut self, id: u64, addr: SocketAddr, timeout: NetTimeout) -> Provided<()> {
        Ok(self.connected(id, &[addr], timeout))
    }
    fn read(&mut self, stream: u64, max: u64) -> Provided<Vec<u8>> {
        if let Some(conn) = self.tls.get_mut(&stream) {
            let fd = self.sockets[&stream].as_raw_fd();
            return Ok(crate::net_tls::read(conn, fd, max));
        }
        Ok(self.socket(stream).and_then(|fd| {
            let mut buffer = vec![0u8; max.min(READ_CHUNK) as usize];
            loop {
                // SAFETY: writable buffer of its length.
                match checked(unsafe {
                    libc::recv(fd, buffer.as_mut_ptr().cast(), buffer.len(), 0)
                }) {
                    Ok(n) => {
                        buffer.truncate(n);
                        return Ok(buffer);
                    }
                    Err(libc::EINTR) => {}
                    Err(code) => return Err(error(code)),
                }
            }
        }))
    }
    fn write(&mut self, stream: u64, bytes: &[u8]) -> Provided<u64> {
        Ok(self.send(stream, bytes))
    }
    fn write_all(&mut self, stream: u64, bytes: &[u8]) -> Provided<()> {
        // Rust `Write::write_all`: a zero-byte write is an error, not a busy loop.
        let mut rest = bytes;
        while !rest.is_empty() {
            match self.send(stream, rest) {
                Ok(0) => return Ok(Err(NetError::BrokenPipe)),
                Ok(n) => rest = &rest[n as usize..],
                Err(e) => return Ok(Err(e)),
            }
        }
        let fd = match self.socket(stream) {
            Ok(fd) => fd,
            Err(e) => return Ok(Err(e)),
        };
        Ok(match self.tls.get_mut(&stream) {
            Some(conn) => crate::net_tls::flush(conn, fd),
            None => Ok(()),
        })
    }
    fn shutdown(&mut self, stream: u64, how: ShutdownHow) -> Provided<()> {
        let how = match how {
            ShutdownHow::Read => libc::SHUT_RD,
            ShutdownHow::Write => libc::SHUT_WR,
            ShutdownHow::Both => libc::SHUT_RDWR,
        };
        Ok(self.socket(stream).and_then(|fd| {
            // SAFETY: plain socket call.
            if unsafe { libc::shutdown(fd, how) } == -1 {
                Err(error(errno()))
            } else {
                Ok(())
            }
        }))
    }
    fn set_timeout(
        &mut self,
        stream: u64,
        which: TimeoutKind,
        timeout: NetTimeout,
    ) -> Provided<()> {
        let option = match which {
            TimeoutKind::Read => libc::SO_RCVTIMEO,
            TimeoutKind::Write => libc::SO_SNDTIMEO,
        };
        Ok(self.set_option(stream, option, timeval(timeout)))
    }
    fn peer_addr(&mut self, stream: u64) -> Provided<SocketAddr> {
        Ok(self
            .socket(stream)
            .and_then(|fd| name_of(fd, libc::getpeername)))
    }
    fn tls_connect(&mut self, stream: u64, name: &str, roots: &RootsDigest) -> Provided<()> {
        let pem = self.pem.take();
        Ok((|| {
            let fd = self.socket(stream)?;
            if self.tls.contains_key(&stream) {
                return Err(NetError::InvalidInput);
            }
            let mut conn = crate::net_tls::client(name, roots, pem)?;
            conn.complete_io(&mut crate::net_tls::Transport(fd))
                .map_err(crate::net_tls::error)?;
            self.tls.insert(stream, conn);
            Ok(())
        })())
    }
    fn tls_close(&mut self, stream: u64) -> Provided<()> {
        Ok((|| {
            let fd = self.socket(stream)?;
            if let Some(conn) = self.tls.get_mut(&stream) {
                conn.send_close_notify();
                crate::net_tls::flush(conn, fd)?;
            }
            Ok(())
        })())
    }
    fn listen(&mut self, id: u64, addr: SocketAddr) -> Provided<()> {
        Ok((|| {
            // Zig `ListenOptions` defaults: no SO_REUSEADDR, backlog 128.
            let socket = new_socket(addr.ip)?;
            let (storage, len) = to_c(addr);
            // SAFETY: storage holds a sockaddr of len bytes.
            if unsafe { libc::bind(socket.as_raw_fd(), (&raw const storage).cast(), len) } == -1
                // SAFETY: plain socket call.
                || unsafe { libc::listen(socket.as_raw_fd(), BACKLOG) } == -1
            {
                return Err(error(errno()));
            }
            assert!(self.sockets.insert(id, socket).is_none());
            Ok(())
        })())
    }
    fn accept(&mut self, listener: u64, id: u64) -> Provided<SocketAddr> {
        Ok(self.socket(listener).and_then(|fd| {
            loop {
                // SAFETY: zeroed storage receives at most len bytes.
                let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                let accepted = unsafe {
                    libc::accept4(fd, (&raw mut storage).cast(), &mut len, libc::SOCK_CLOEXEC)
                };
                if accepted == -1 {
                    match errno() {
                        libc::EINTR => continue,
                        code => return Err(error(code)),
                    }
                }
                // SAFETY: a fresh descriptor owned by nobody else.
                let socket = unsafe { OwnedFd::from_raw_fd(accepted) };
                let peer = from_c(&storage)?;
                assert!(self.sockets.insert(id, socket).is_none());
                return Ok(peer);
            }
        }))
    }
    fn local_addr(&mut self, listener: u64) -> Provided<SocketAddr> {
        Ok(self
            .socket(listener)
            .and_then(|fd| name_of(fd, libc::getsockname)))
    }
    fn close(&mut self, handle: u64) {
        // DROP-3: dropping TLS never sends close_notify or blocks.
        self.tls.remove(&handle);
        // OwnedFd's drop discards the close error (DROP-3).
        self.sockets.remove(&handle);
    }
}
