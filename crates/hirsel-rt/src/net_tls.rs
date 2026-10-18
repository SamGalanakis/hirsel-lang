//! TLS provider boundary, following rustls 0.23.46 src/stream.rs (Stream),
//! src/conn.rs (complete_io), src/client/client_conn.rs, src/server/server_conn.rs and
//! src/error.rs; SNI selection and client verification follow src/server/handy.rs and
//! src/webpki/client_verifier.rs.
//! Socket ownership and grants stay in HostNet/State; this module never records
//! TLS records, keys, random bytes or time.
use crate::net::{NetError, RootsDigest, TlsFailure};
use hirsel_run_config::tls::Server;
use rustls::{
    ClientConfig, ClientConnection, Connection, RootCertStore, ServerConfig, ServerConnection,
};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::sync::Arc;

/// Tape identity of the compiled Mozilla root set; update with Cargo.toml.
pub const WEBPKI_VERSION: &str = "webpki-roots@1.0.9";

pub fn pem_digest(pem: &[u8]) -> RootsDigest {
    RootsDigest::Pem(chain_digest(pem))
}

/// The tape identity of a `tls.accept` identity: the SHA-256 of its certificate-chain PEM.
pub fn chain_digest(pem: &[u8]) -> [u8; 32] {
    Sha256::digest(pem).into()
}

/// The ALPN protocols a server handshake offers (docs/spec/core-library.md).
pub const SERVER_ALPN: &[&[u8]] = &[b"http/1.1"];

/// Presents the identity `tls::select` picks for the client's SNI.
#[derive(Debug)]
struct BySni(Vec<Arc<rustls::sign::CertifiedKey>>);
impl rustls::server::ResolvesServerCert for BySni {
    fn resolve(
        &self,
        hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(hirsel_run_config::tls::select(&self.0, hello.server_name()).clone())
    }
}

/// A server handshake with a server the shared argument checks loaded: the identity the
/// client's SNI selects, and the client verifier of its trust set. No session cache or
/// tickets.
pub(crate) fn server(server: &Server) -> Result<ServerConnection, NetError> {
    let builder =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| NetError::Tls(failure(&e)))?;
    let builder = match &server.clients {
        Some(verifier) => builder.with_client_cert_verifier(verifier.clone()),
        None => builder.with_no_client_auth(),
    };
    let mut config = builder.with_cert_resolver(Arc::new(BySni(server.identities.clone())));
    config.alpn_protocols = SERVER_ALPN.iter().map(|p| p.to_vec()).collect();
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    ServerConnection::new(Arc::new(config)).map_err(|e| NetError::Tls(failure(&e)))
}

/// What `tls.peer` returns for a connection: the chain a server verified, never a client's.
pub(crate) fn peer(conn: &Connection) -> Option<Vec<u8>> {
    match conn {
        Connection::Server(conn) => conn
            .peer_certificates()
            .map(hirsel_run_config::tls::chain_der),
        Connection::Client(_) => None,
    }
}

/// The host provider verifies with the trust anchors parsed by the shared argument checks,
/// and presents `identity` when the server asks for a client certificate. No cached config
/// enables resumption or ALPN.
pub(crate) fn client(
    name: &str,
    roots: &RootsDigest,
    pem: Option<Vec<u8>>,
    identity: Option<&Arc<rustls::sign::CertifiedKey>>,
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
                roots: hirsel_run_config::tls::pem_roots(&pem).ok_or(NetError::InvalidInput)?,
            }
        }
        _ => return Err(NetError::InvalidInput),
    };
    let name = hirsel_run_config::tls::server_name(name).ok_or(NetError::InvalidInput)?;
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
    ClientConnection::new(Arc::new(config), name).map_err(|e| NetError::Tls(failure(&e)))
}

/// Condense rustls Error/CertificateError into the runtime's TLS errors.
pub fn failure(error: &rustls::Error) -> TlsFailure {
    hirsel_run_config::tls::failure(error).into()
}

pub(crate) fn error(error: io::Error) -> NetError {
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
pub(crate) struct Transport(pub libc::c_int);
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
pub(crate) fn read(conn: &mut Connection, fd: libc::c_int, max: u64) -> Result<Vec<u8>, NetError> {
    let mut bytes = vec![0; max.min(crate::net_host::READ_CHUNK) as usize];
    let n = plaintext(conn, fd, |s| s.read(&mut bytes)).map_err(error)?;
    bytes.truncate(n);
    Ok(bytes)
}
pub(crate) fn write(conn: &mut Connection, fd: libc::c_int, bytes: &[u8]) -> Result<u64, NetError> {
    plaintext(conn, fd, |s| s.write(bytes))
        .map(|n| n as u64)
        .map_err(error)
}
pub(crate) fn flush(conn: &mut Connection, fd: libc::c_int) -> Result<(), NetError> {
    plaintext(conn, fd, |s| s.flush()).map_err(error)
}
