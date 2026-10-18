//! TLS provider boundary, following rustls 0.23.46 src/stream.rs (Stream),
//! src/conn.rs (complete_io), src/client/client_conn.rs and src/error.rs.
//! Socket ownership and grants stay in HostNet/State; this module never records
//! TLS records, keys, random bytes or time.
use crate::net::{NetError, RootsDigest, TlsFailure};
use rustls::{ClientConfig, ClientConnection, RootCertStore};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::sync::Arc;

/// Tape identity of the compiled Mozilla root set; update with Cargo.toml.
pub const WEBPKI_VERSION: &str = "webpki-roots@1.0.9";

pub fn pem_digest(pem: &[u8]) -> RootsDigest {
    RootsDigest::Pem(Sha256::digest(pem).into())
}

/// The host provider verifies with the trust anchors parsed by the shared argument checks.
/// No cached config enables
/// resumption, client auth or ALPN.
pub(crate) fn client(
    name: &str,
    roots: &RootsDigest,
    pem: Option<Vec<u8>>,
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
    let mut config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| NetError::Tls(failure(&e)))?
            .with_root_certificates(roots)
            .with_no_client_auth();
    config.resumption = rustls::client::Resumption::disabled();
    ClientConnection::new(Arc::new(config), name).map_err(|e| NetError::Tls(failure(&e)))
}

/// Condense rustls Error/CertificateError into the runtime's TLS errors.
pub fn failure(error: &rustls::Error) -> TlsFailure {
    use rustls::{CertificateError as C, Error as E};
    match error {
        E::InvalidCertificate(cert) => match cert {
            C::UnknownIssuer => TlsFailure::UnknownIssuer,
            C::Expired | C::ExpiredContext { .. } => TlsFailure::Expired,
            C::NotValidYet | C::NotValidYetContext { .. } => TlsFailure::NotYetValid,
            C::NotValidForName | C::NotValidForNameContext { .. } => TlsFailure::NameMismatch,
            C::Revoked => TlsFailure::Revoked,
            _ => TlsFailure::BadCertificate,
        },
        E::NoCertificatesPresented => TlsFailure::BadCertificate,
        E::AlertReceived(alert) => TlsFailure::AlertReceived(u8::from(*alert)),
        E::PeerMisbehaved(_)
        | E::InappropriateMessage { .. }
        | E::InappropriateHandshakeMessage { .. }
        | E::InvalidMessage(_)
        | E::PeerSentOversizedRecord
        | E::DecryptError => TlsFailure::PeerMisbehaved,
        _ => TlsFailure::HandshakeFailure,
    }
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

pub(crate) fn read(
    conn: &mut ClientConnection,
    fd: libc::c_int,
    max: u64,
) -> Result<Vec<u8>, NetError> {
    let mut bytes = vec![0; max.min(crate::net_host::READ_CHUNK) as usize];
    let n = rustls::Stream::new(conn, &mut Transport(fd))
        .read(&mut bytes)
        .map_err(error)?;
    bytes.truncate(n);
    Ok(bytes)
}
pub(crate) fn write(
    conn: &mut ClientConnection,
    fd: libc::c_int,
    bytes: &[u8],
) -> Result<u64, NetError> {
    rustls::Stream::new(conn, &mut Transport(fd))
        .write(bytes)
        .map(|n| n as u64)
        .map_err(error)
}
pub(crate) fn flush(conn: &mut ClientConnection, fd: libc::c_int) -> Result<(), NetError> {
    rustls::Stream::new(conn, &mut Transport(fd))
        .flush()
        .map_err(error)
}
