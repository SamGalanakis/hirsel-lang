//! The transport of `remote.tls@1`: TLS over a TCP or a Unix stream socket, so one
//! authentication story covers both (RUN-13). Client and server configurations follow
//! rustls 0.23.46 (`ClientConfig::builder_with_provider`, `ServerConfig::with_cert_resolver`,
//! `WebPkiClientVerifier`) with the ring provider, as the program-facing TLS provider does
//! (hirsel-rt `net_tls.rs`); the material loaders are hirsel-run-schema's.
//!
//! A [`Connection`] owns one socket and its TLS session on its own thread: it writes the
//! frames handed to it and reads frames as they arrive, so a caller never blocks on the
//! network. The socket has a short read timeout, which bounds how long a queued write
//! waits for the reader to yield.
use crate::frame::Frame;
use crate::wire::Wire;
use hirsel_run_schema::Endpoint;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

/// How long a connection's reader waits before it looks for frames to write.
const POLL: Duration = Duration::from_millis(5);
/// The bound on connecting and on a TLS handshake.
const HANDSHAKE: Duration = Duration::from_secs(10);

/// A connected stream socket of either family.
pub enum Socket {
    Tcp(TcpStream),
    Unix(UnixStream),
}
impl Socket {
    pub fn connect(endpoint: &Endpoint) -> io::Result<Self> {
        Ok(match endpoint {
            Endpoint::Tcp { address, service } => {
                let port: u16 = service
                    .parse()
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
                let mut last = io::Error::from(io::ErrorKind::NotFound);
                let mut connected = None;
                for addr in std::net::ToSocketAddrs::to_socket_addrs(&(address.as_str(), port))? {
                    match TcpStream::connect_timeout(&addr, HANDSHAKE) {
                        Ok(stream) => {
                            connected = Some(stream);
                            break;
                        }
                        Err(error) => last = error,
                    }
                }
                let stream = connected.ok_or(last)?;
                stream.set_nodelay(true)?;
                Self::Tcp(stream)
            }
            Endpoint::Unix(path) => Self::Unix(UnixStream::connect(path)?),
        })
    }
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Tcp(s) => s.set_read_timeout(timeout),
            Self::Unix(s) => s.set_read_timeout(timeout),
        }
    }
    fn shutdown(&self) {
        let _ = match self {
            Self::Tcp(s) => s.shutdown(std::net::Shutdown::Both),
            Self::Unix(s) => s.shutdown(std::net::Shutdown::Both),
        };
    }
}
impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(s) => s.read(buf),
            Self::Unix(s) => s.read(buf),
        }
    }
}
impl Write for Socket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(s) => s.write(buf),
            Self::Unix(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Tcp(s) => s.flush(),
            Self::Unix(s) => s.flush(),
        }
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}
fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what.to_owned())
}

/// A caller's TLS client: the host's name, the roots it is verified against and the
/// identity presented when the host asks for one, all read once.
pub struct ClientTls {
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
}
impl ClientTls {
    /// `roots` is a PEM bundle, or None for the built-in webpki set; `identity` a PEM chain
    /// and key.
    pub fn new(
        server_name: &str,
        roots: Option<&[u8]>,
        identity: Option<(&[u8], &[u8])>,
    ) -> io::Result<Self> {
        let name = hirsel_run_schema::tls::server_name(server_name)
            .ok_or_else(|| invalid("the server name"))?;
        let roots = match roots {
            None => RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            },
            Some(pem) => RootCertStore {
                roots: hirsel_run_schema::tls::pem_roots(pem)
                    .ok_or_else(|| invalid("the trust anchors"))?,
            },
        };
        let builder = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|_| invalid("TLS protocol versions"))?
            .with_root_certificates(roots);
        let config = match identity {
            None => builder.with_no_client_auth(),
            Some((chain, key)) => {
                let identity = hirsel_run_schema::tls::identity(chain, key)
                    .ok_or_else(|| invalid("the client identity"))?;
                builder.with_client_cert_resolver(Arc::new(rustls::sign::SingleCertAndKey::from(
                    identity,
                )))
            }
        };
        let mut config = config;
        config.resumption = rustls::client::Resumption::disabled();
        Ok(Self {
            config: Arc::new(config),
            name,
        })
    }
    /// Connect to `endpoint` and complete the handshake.
    pub fn connect(&self, endpoint: &Endpoint) -> io::Result<Tls> {
        let mut socket = Socket::connect(endpoint)?;
        socket.set_read_timeout(Some(HANDSHAKE))?;
        let mut session = ClientConnection::new(self.config.clone(), self.name.clone())
            .map_err(io::Error::other)?;
        while session.is_handshaking() {
            session.complete_io(&mut socket)?;
        }
        Ok(Tls {
            session: rustls::Connection::Client(session),
            socket,
        })
    }
}

/// A runner host's TLS server: its identity and the client trust anchors.
pub struct HostTls {
    config: Arc<ServerConfig>,
}
/// What a client's handshake proved: its end-entity certificate's SHA-256 and DER.
pub struct Peer {
    pub digest: [u8; 32],
    pub certificate: Vec<u8>,
}
impl Peer {
    /// Whether the certificate names `name` (a DNS or IP subject alternative name).
    pub fn names(&self, name: &str) -> bool {
        let der = CertificateDer::from(self.certificate.as_slice());
        let (Some(name), Ok(cert)) = (
            hirsel_run_schema::tls::server_name(name),
            webpki::EndEntityCert::try_from(&der),
        ) else {
            return false;
        };
        cert.verify_is_valid_for_subject_name(&name).is_ok()
    }
}
impl HostTls {
    /// `clients` is the PEM bundle client certificates must chain to, or None to serve
    /// clients without certificates.
    pub fn new(chain: &[u8], key: &[u8], clients: Option<&[u8]>) -> io::Result<Self> {
        let identity = hirsel_run_schema::tls::identity(chain, key)
            .ok_or_else(|| invalid("the host identity"))?;
        let builder = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|_| invalid("TLS protocol versions"))?;
        let builder = match clients {
            None => builder.with_no_client_auth(),
            Some(pem) => {
                let trust = hirsel_run_schema::tls::pem_roots(pem)
                    .ok_or_else(|| invalid("the client trust anchors"))?;
                let verifier = hirsel_run_schema::tls::client_verifier(trust, true)
                    .ok_or_else(|| invalid("the client trust anchors"))?;
                builder.with_client_cert_verifier(verifier)
            }
        };
        let mut config =
            builder.with_cert_resolver(Arc::new(rustls::sign::SingleCertAndKey::from(identity)));
        config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
        config.send_tls13_tickets = 0;
        Ok(Self {
            config: Arc::new(config),
        })
    }
    /// Complete a server handshake on an accepted socket: the session and the client's
    /// certificate, when it presented one.
    pub fn accept(&self, socket: Socket) -> io::Result<(Tls, Option<Peer>)> {
        let mut socket = socket;
        socket.set_read_timeout(Some(HANDSHAKE))?;
        let mut session = ServerConnection::new(self.config.clone()).map_err(io::Error::other)?;
        while session.is_handshaking() {
            session.complete_io(&mut socket)?;
        }
        let peer = session
            .peer_certificates()
            .and_then(|chain| chain.first())
            .map(|leaf| Peer {
                digest: Sha256::digest(leaf.as_ref()).into(),
                certificate: leaf.to_vec(),
            });
        Ok((
            Tls {
                session: rustls::Connection::Server(session),
                socket,
            },
            peer,
        ))
    }
}

/// A host's listening socket.
pub enum Listener {
    Tcp(std::net::TcpListener),
    Unix(UnixListener),
}
impl Listener {
    /// Listen at `endpoint`. A Unix socket path whose socket no longer accepts connections
    /// (a previous host's) is replaced; one a live host serves is refused.
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        Ok(match endpoint {
            Endpoint::Tcp { address, service } => {
                let port: u16 = service.parse().map_err(|_| invalid("the service"))?;
                Self::Tcp(std::net::TcpListener::bind((address.as_str(), port))?)
            }
            Endpoint::Unix(path) => match UnixListener::bind(path) {
                Ok(listener) => Self::Unix(listener),
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                    if UnixStream::connect(path).is_ok() {
                        return Err(error);
                    }
                    std::fs::remove_file(path)?;
                    Self::Unix(UnixListener::bind(path)?)
                }
                Err(error) => return Err(error),
            },
        })
    }
    pub fn accept(&self) -> io::Result<Socket> {
        Ok(match self {
            Self::Tcp(l) => {
                let (stream, _) = l.accept()?;
                stream.set_nodelay(true)?;
                Socket::Tcp(stream)
            }
            Self::Unix(l) => Socket::Unix(l.accept()?.0),
        })
    }
}

/// One TLS session over its socket.
pub struct Tls {
    session: rustls::Connection,
    socket: Socket,
}

/// What a connection thread reports.
pub enum Event {
    Frame(Frame),
    /// The connection ended: closed by either side, failed, or a frame did not decode.
    Closed,
}
/// What a connection thread is asked to do.
pub enum Command {
    Send(Frame),
    Close,
}

/// Run a connection until it closes: write every frame `commands` hands over, and report
/// every frame that arrives, then [`Event::Closed`], through `report`. `report` returns
/// false once nobody listens, which also ends the connection.
pub fn serve(
    tls: Tls,
    wire: Arc<Wire>,
    commands: Receiver<Command>,
    mut report: impl FnMut(Event) -> bool,
) {
    let Tls {
        session,
        mut socket,
    } = tls;
    let _ = socket.set_read_timeout(Some(POLL));
    match session {
        rustls::Connection::Client(mut session) => {
            pump(&mut session, &mut socket, &wire, &commands, &mut report)
        }
        rustls::Connection::Server(mut session) => {
            pump(&mut session, &mut socket, &wire, &commands, &mut report)
        }
    }
    socket.shutdown();
    report(Event::Closed);
}

fn pump<C, S>(
    session: &mut C,
    socket: &mut Socket,
    wire: &Wire,
    commands: &Receiver<Command>,
    report: &mut impl FnMut(Event) -> bool,
) where
    C: std::ops::DerefMut + std::ops::Deref<Target = rustls::ConnectionCommon<S>>,
    S: rustls::SideData,
{
    let mut stream = rustls::Stream::new(session, socket);
    let mut buffer: Vec<u8> = vec![];
    let mut chunk = vec![0u8; 64 * 1024];
    'connection: loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Send(frame)) => {
                    if wire.write(&mut stream, &frame).is_err() {
                        break 'connection;
                    }
                }
                Ok(Command::Close) | Err(TryRecvError::Disconnected) => break 'connection,
                Err(TryRecvError::Empty) => break,
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => break,
        }
        // Every whole frame the buffer holds.
        while buffer.len() >= 8 {
            let len = u64::from_le_bytes(buffer[..8].try_into().unwrap());
            if len > wire.limit() {
                break 'connection;
            }
            let end = 8 + len as usize;
            if buffer.len() < end {
                break;
            }
            let frame = wire.decode(&buffer[8..end]);
            buffer.drain(..end);
            match frame {
                Ok(frame) => {
                    if !report(Event::Frame(frame)) {
                        break 'connection;
                    }
                }
                // A frame that does not decode ends the session: nothing after it can be
                // trusted to be framed.
                Err(_) => break 'connection,
            }
        }
    }
    stream.conn.send_close_notify();
    let _ = stream.conn.complete_io(stream.sock);
}
