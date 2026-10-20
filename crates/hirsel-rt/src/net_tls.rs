//! TLS provider boundary, following rustls 0.23.46 src/stream.rs (Stream),
//! src/conn.rs (complete_io), src/client/client_conn.rs, src/server/server_conn.rs and
//! src/error.rs; SNI selection and client verification follow src/server/handy.rs and
//! src/webpki/client_verifier.rs.
//! Socket ownership and grants stay in HostNet/State; this module never records
//! TLS records, keys, random bytes or time.
//!
//! The implementation is reached only through [`TlsEngine`] and [`TlsSession`] objects, and
//! only the TLS entries (`tls_abi.rs`, and the interpreter's TLS operations) name the engine:
//! they hand it to the adapter before their request. The runtime archive keeps one function
//! per section and the link collects unreferenced sections, so a program that calls no TLS
//! entry links none of rustls, ring, webpki or the root set.
use crate::net::{NetError, RootsDigest, TlsFailure};
use hirsel_run_schema::tls::{ClientAuth, Identity, Server};
use rustls::{
    ClientConfig, ClientConnection, Connection, RootCertStore, ServerConfig, ServerConnection,
};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::sync::Arc;

/// TLS material checks and handshake starts: everything of TLS that does not belong to one
/// connection. The policy layer and every world (host, scripted, replay) reach TLS only
/// through the engine their adapter was handed.
pub trait TlsEngine {
    /// Whether `name` is a valid verification/SNI name (no resolver or clock).
    fn valid_name(&self, name: &str) -> bool;
    /// Whether `pem` is a nonempty bundle of well-formed trust anchors.
    fn valid_roots(&self, pem: &[u8]) -> bool;
    /// A client or server identity: its PEM chain and key, when valid and matching.
    fn identity(&self, chain: &[u8], key: &[u8]) -> Option<Arc<Identity>>;
    /// A `TlsServer`: its identities, client policy and ALPN list, when the whole server is
    /// valid.
    fn server(
        &self,
        identities: &[(&[u8], &[u8])],
        clients: ClientAuth<&[u8]>,
        alpn: &[String],
    ) -> Option<Server>;
    /// A client connection verifying `name` against `roots` (the Pem material in `pem`),
    /// presenting `identity` when the server asks for a certificate and offering the ALPN
    /// protocols `alpn` in order.
    fn connect(
        &self,
        name: &str,
        roots: &RootsDigest,
        pem: Option<Vec<u8>>,
        identity: Option<&Arc<Identity>>,
        alpn: &[String],
    ) -> Result<Box<dyn TlsSession>, NetError>;
    /// A server connection presenting the identity the client's SNI selects.
    fn accept(&self, server: &Server) -> Result<Box<dyn TlsSession>, NetError>;
    /// A scripted client's answer to a server handshake: the chain it presented and the
    /// protocol negotiated (`hirsel_run_schema::tls::scripted_accept`).
    fn scripted_accept(
        &self,
        client: &hirsel_run_schema::PeerTls,
        server: &Server,
    ) -> Result<hirsel_run_schema::tls::Handshake, hirsel_run_schema::TlsFailure>;
    /// A scripted peer's answer to a client handshake offering `alpn`: the chain it
    /// presented and the protocol negotiated (`hirsel_run_schema::tls::scripted_connect`).
    fn scripted_connect(
        &self,
        peer: &hirsel_run_schema::PeerTls,
        server_name: &str,
        identity: Option<&Identity>,
        alpn: &[String],
    ) -> Result<hirsel_run_schema::tls::Handshake, hirsel_run_schema::TlsFailure>;
}

/// An adapter's TLS engine, once a TLS entry handed it over. An engine holds no state, so
/// any two compare equal.
#[derive(Clone, Copy, Default)]
pub(crate) struct Engine(pub(crate) Option<&'static dyn TlsEngine>);
impl PartialEq for Engine {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for Engine {}
impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "TlsEngine"
        } else {
            "NoTls"
        })
    }
}

/// Why a nonblocking TLS step stopped short.
pub enum Stall {
    /// The socket would block; readiness resumes the step.
    WouldBlock,
    Failed(NetError),
}

/// One TLS connection over a socket its host provider owns, in rustls's terms. Blocking
/// providers use the `Stream` operations; the cooperative provider drives the record layer
/// itself across readiness waits.
pub trait TlsSession {
    fn is_handshaking(&self) -> bool;
    fn wants_write(&self) -> bool;
    /// Encrypted bytes from the socket; 0 at its end.
    fn read_tls(&mut self, fd: libc::c_int) -> Result<usize, Stall>;
    fn write_tls(&mut self, fd: libc::c_int) -> Result<usize, Stall>;
    fn process_new_packets(&mut self) -> Result<(), TlsFailure>;
    /// Buffered plaintext; `WouldBlock` when none is buffered.
    fn read_plaintext(&mut self, bytes: &mut [u8]) -> Result<usize, Stall>;
    /// Plaintext into the bounded send buffer.
    fn write_plaintext(&mut self, bytes: &[u8]) -> Result<usize, NetError>;
    fn send_close_notify(&mut self);
    /// Finish the handshake over a blocking socket.
    fn complete_io(&mut self, fd: libc::c_int) -> Result<(), NetError>;
    fn read(&mut self, fd: libc::c_int, max: u64) -> Result<Vec<u8>, NetError>;
    fn write(&mut self, fd: libc::c_int, bytes: &[u8]) -> Result<u64, NetError>;
    fn flush(&mut self, fd: libc::c_int) -> Result<(), NetError>;
    /// What `tls.peer` returns: the chain its peer presented and the handshake verified,
    /// the server's on a client and a client's, if any, on a server.
    fn peer(&self) -> Option<Vec<u8>>;
    /// The ALPN protocol a finished handshake negotiated. Both sides only ever agree on a
    /// protocol one of them listed, and every listed protocol is a String.
    fn protocol(&self) -> Option<String>;
}
impl std::fmt::Debug for dyn TlsSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TlsSession")
    }
}

/// The runtime's TLS engine: rustls with the ring provider and the compiled root set.
pub fn engine() -> &'static dyn TlsEngine {
    &Rustls
}

struct Rustls;
impl TlsEngine for Rustls {
    fn valid_name(&self, name: &str) -> bool {
        hirsel_run_schema::tls::server_name(name).is_some()
    }
    fn valid_roots(&self, pem: &[u8]) -> bool {
        hirsel_run_schema::tls::pem_roots(pem).is_some()
    }
    fn identity(&self, chain: &[u8], key: &[u8]) -> Option<Arc<Identity>> {
        hirsel_run_schema::tls::identity(chain, key).map(Arc::new)
    }
    fn server(
        &self,
        identities: &[(&[u8], &[u8])],
        clients: ClientAuth<&[u8]>,
        alpn: &[String],
    ) -> Option<Server> {
        hirsel_run_schema::tls::server(identities, clients, alpn)
    }
    fn connect(
        &self,
        name: &str,
        roots: &RootsDigest,
        pem: Option<Vec<u8>>,
        identity: Option<&Arc<Identity>>,
        alpn: &[String],
    ) -> Result<Box<dyn TlsSession>, NetError> {
        client(name, roots, pem, identity, alpn).map(|c| Box::new(Connection::from(c)) as _)
    }
    fn accept(&self, server: &Server) -> Result<Box<dyn TlsSession>, NetError> {
        self::server(server).map(|c| Box::new(Connection::from(c)) as _)
    }
    fn scripted_accept(
        &self,
        client: &hirsel_run_schema::PeerTls,
        server: &Server,
    ) -> Result<hirsel_run_schema::tls::Handshake, hirsel_run_schema::TlsFailure> {
        hirsel_run_schema::tls::scripted_accept(client, server)
    }
    fn scripted_connect(
        &self,
        peer: &hirsel_run_schema::PeerTls,
        server_name: &str,
        identity: Option<&Identity>,
        alpn: &[String],
    ) -> Result<hirsel_run_schema::tls::Handshake, hirsel_run_schema::TlsFailure> {
        hirsel_run_schema::tls::scripted_connect(peer, server_name, identity, alpn)
    }
}

fn stall(error: io::Error) -> Stall {
    if error.kind() == io::ErrorKind::WouldBlock {
        Stall::WouldBlock
    } else {
        Stall::Failed(self::error(error))
    }
}

impl TlsSession for Connection {
    // The connection's common state (`Deref`) holds these three.
    fn is_handshaking(&self) -> bool {
        (**self).is_handshaking()
    }
    fn wants_write(&self) -> bool {
        (**self).wants_write()
    }
    fn read_tls(&mut self, fd: libc::c_int) -> Result<usize, Stall> {
        Connection::read_tls(self, &mut Transport(fd)).map_err(stall)
    }
    fn write_tls(&mut self, fd: libc::c_int) -> Result<usize, Stall> {
        Connection::write_tls(self, &mut Transport(fd)).map_err(stall)
    }
    fn process_new_packets(&mut self) -> Result<(), TlsFailure> {
        Connection::process_new_packets(self)
            .map(|_| ())
            .map_err(|e| failure(&e))
    }
    fn read_plaintext(&mut self, bytes: &mut [u8]) -> Result<usize, Stall> {
        self.reader().read(bytes).map_err(stall)
    }
    fn write_plaintext(&mut self, bytes: &[u8]) -> Result<usize, NetError> {
        self.writer().write(bytes).map_err(error)
    }
    fn send_close_notify(&mut self) {
        (**self).send_close_notify();
    }
    fn complete_io(&mut self, fd: libc::c_int) -> Result<(), NetError> {
        Connection::complete_io(self, &mut Transport(fd))
            .map(|_| ())
            .map_err(error)
    }
    fn read(&mut self, fd: libc::c_int, max: u64) -> Result<Vec<u8>, NetError> {
        let mut bytes = vec![0; max.min(crate::net_host::READ_CHUNK) as usize];
        let n = plaintext(self, fd, |s| s.read(&mut bytes)).map_err(error)?;
        bytes.truncate(n);
        Ok(bytes)
    }
    fn write(&mut self, fd: libc::c_int, bytes: &[u8]) -> Result<u64, NetError> {
        plaintext(self, fd, |s| s.write(bytes))
            .map(|n| n as u64)
            .map_err(error)
    }
    fn flush(&mut self, fd: libc::c_int) -> Result<(), NetError> {
        plaintext(self, fd, |s| s.flush()).map_err(error)
    }
    fn peer(&self) -> Option<Vec<u8>> {
        self.peer_certificates()
            .map(hirsel_run_schema::tls::chain_der)
    }
    fn protocol(&self) -> Option<String> {
        self.alpn_protocol()
            .map(|p| String::from_utf8_lossy(p).into_owned())
    }
}

/// Tape identity of the compiled Mozilla root set; update with Cargo.toml.
pub const WEBPKI_VERSION: &str = "webpki-roots@1.0.9";

pub fn pem_digest(pem: &[u8]) -> RootsDigest {
    RootsDigest::Pem(chain_digest(pem))
}

/// The tape identity of a `tls.accept` identity: the SHA-256 of its certificate-chain PEM.
pub fn chain_digest(pem: &[u8]) -> [u8; 32] {
    Sha256::digest(pem).into()
}

/// Presents the identity `tls::select` picks for the client's SNI.
#[derive(Debug)]
struct BySni(Vec<Arc<Identity>>);
impl rustls::server::ResolvesServerCert for BySni {
    fn resolve(&self, hello: rustls::server::ClientHello<'_>) -> Option<Arc<Identity>> {
        Some(hirsel_run_schema::tls::select(&self.0, hello.server_name()).clone())
    }
}

/// A server handshake with a server the shared argument checks loaded: the identity the
/// client's SNI selects, the client verifier of its trust set and its ALPN list, which
/// rustls selects from in the server's order. No session cache or tickets.
fn server(server: &Server) -> Result<ServerConnection, NetError> {
    let builder =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| NetError::Tls(failure(&e)))?;
    let builder = match &server.clients {
        Some(verifier) => builder.with_client_cert_verifier(verifier.clone()),
        None => builder.with_no_client_auth(),
    };
    let mut config = builder.with_cert_resolver(Arc::new(BySni(server.identities.clone())));
    config.alpn_protocols = alpn(&server.alpn);
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    ServerConnection::new(Arc::new(config)).map_err(|e| NetError::Tls(failure(&e)))
}

fn alpn(protocols: &[String]) -> Vec<Vec<u8>> {
    protocols.iter().map(|p| p.as_bytes().to_vec()).collect()
}

/// The host provider verifies with the trust anchors parsed by the shared argument checks,
/// presents `identity` when the server asks for a client certificate and offers the ALPN
/// protocols `protocols` in order. No cached config enables resumption.
fn client(
    name: &str,
    roots: &RootsDigest,
    pem: Option<Vec<u8>>,
    identity: Option<&Arc<Identity>>,
    protocols: &[String],
) -> Result<ClientConnection, NetError> {
    let roots = match roots {
        RootsDigest::WebPki(version) if version == WEBPKI_VERSION => RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        },
        RootsDigest::Pem(_) => {
            let pem = pem.ok_or(NetError::InvalidInput)?;
            if pem_digest(&pem) != *roots {
                return Err(NetError::InvalidInput);
            }
            RootCertStore {
                roots: hirsel_run_schema::tls::pem_roots(&pem).ok_or(NetError::InvalidInput)?,
            }
        }
        _ => return Err(NetError::InvalidInput),
    };
    let name = hirsel_run_schema::tls::server_name(name).ok_or(NetError::InvalidInput)?;
    let builder =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| NetError::Tls(failure(&e)))?
            .with_root_certificates(roots);
    let mut config = match identity {
        Some(identity) => builder.with_client_cert_resolver(Arc::new(
            rustls::sign::SingleCertAndKey::from(identity.clone()),
        )),
        None => builder.with_no_client_auth(),
    };
    config.resumption = rustls::client::Resumption::disabled();
    config.alpn_protocols = alpn(protocols);
    ClientConnection::new(Arc::new(config), name).map_err(|e| NetError::Tls(failure(&e)))
}

/// Condense rustls Error/CertificateError into the runtime's TLS errors.
pub fn failure(error: &rustls::Error) -> TlsFailure {
    hirsel_run_schema::tls::failure(error).into()
}

fn error(error: io::Error) -> NetError {
    if let Some(tls) = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<rustls::Error>())
    {
        return NetError::Tls(failure(tls));
    }
    if error.kind() == io::ErrorKind::UnexpectedEof {
        return NetError::Tls(TlsFailure::Truncated);
    }
    error
        .raw_os_error()
        .map(crate::net_host::error)
        .unwrap_or(NetError::Fault(1))
}

/// Borrow an already granted socket. Rust std's Unix socket conventions:
/// retry EINTR and suppress SIGPIPE for writes. Nothing owns or duplicates fd.
struct Transport(libc::c_int);
impl Read for Transport {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            // SAFETY: bytes is writable for its length and fd is borrowed from HostNet.
            let n = unsafe { libc::recv(self.0, bytes.as_mut_ptr().cast(), bytes.len(), 0) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}
impl Write for Transport {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            // SAFETY: bytes is readable for its length; the socket remains owned by HostNet.
            let n = unsafe {
                libc::send(
                    self.0,
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    libc::MSG_NOSIGNAL,
                )
            };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

trait Plaintext: Read + Write {}
impl<T: Read + Write> Plaintext for T {}
/// rustls::Stream over either side of a connection.
fn plaintext<R>(
    conn: &mut Connection,
    fd: libc::c_int,
    f: impl FnOnce(&mut dyn Plaintext) -> R,
) -> R {
    let mut transport = Transport(fd);
    match conn {
        Connection::Client(conn) => f(&mut rustls::Stream::new(conn, &mut transport)),
        Connection::Server(conn) => f(&mut rustls::Stream::new(conn, &mut transport)),
    }
}
