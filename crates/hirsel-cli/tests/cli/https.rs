//! HTTPS on the real host (core tls.accept and std http.serve_tls): a program listening
//! under a ListenTls grant serves one request to a verifying rustls client that trusts only
//! the self-signed identity, records the run, and the tape replays in either engine with no
//! client and no key material.
use rustls::pki_types::pem::PemObject;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

const CHAIN: &[u8] = include_bytes!("../../../hirsel-rt/tests/it/tls-data/server.cert");
const KEY: &[u8] = include_bytes!("../../../hirsel-rt/tests/it/tls-data/server.key");

fn literal(pem: &[u8]) -> String {
    format!(
        "b\"{}\"",
        String::from_utf8(pem.to_vec())
            .unwrap()
            .replace('\n', "\\n")
    )
}

fn program() -> String {
    format!(
        r#"pub type ServerError = Grant(GrantError) | Network(NetError) | Http(http.HttpError) | Output(OutputError)

fn identity() -> TlsIdentity uses {{}} {{
  TlsIdentity(chain: {}, key: {})
}}

pub fn main(rt: Io, args: Args) -> Result[(), ServerError] {{
  let n = io.net(rt) ? Grant
  let output = io.stdout(rt) ? Grant
  let mut listener = net.listen(n, SocketAddr(ip: V4([127u8, 0u8, 0u8, 1u8]), port: 0u16)) ? Network
  let bound = net.local_addr(n, listener) ? Network
  out.print(output, std@1/u16.to_string(bound.port)) ? Output
  out.print(output, "\n") ? Output
  let (stream, _) = net.accept(n, mut listener) ? Network
  let mut connection = http.serve_tls(n, take stream, TlsServer(identities: [identity()], clients: Anonymous()), http.default_config()) ? Http
  let request = http.receive_request(mut connection, None()) ? Http
  http.respond(mut connection, 200u16, [], request.body, true) ? Http
  Ok(())
}}
"#,
        literal(CHAIN),
        literal(KEY)
    )
}

const RESPONSE: &[u8] = b"HTTP/1.1 200 \r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello";

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("hirsel-https-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.hir"), program()).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"https","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join("grants.json"),
            serde_json::json!({"io": {"stdout": "stdout.bytes@1", "dirs": [], "net": {
                "provider": "net.stream@1",
                "rules": [{"address": "127.0.0.1", "service": "0", "operation": "listen_tls"}]
            }}, "args": []})
            .to_string(),
        )
        .unwrap();
        Self(root)
    }
    fn command(&self, native: Option<&str>) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.arg("run").arg(self.0.join("main.hir.json"));
        if let Some(backend) = native {
            command.args(["--native", "--backend", backend]);
        }
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A verifying client: it trusts only the self-signed chain, names localhost and offers
/// h2 before http/1.1. Returns the response and the negotiated protocol.
fn fetch(port: u16) -> (Vec<u8>, Vec<u8>) {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls::pki_types::CertificateDer::pem_slice_iter(CHAIN) {
        roots.add(cert.unwrap()).unwrap();
    }
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let socket = std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
    let mut tls = rustls::StreamOwned::new(conn, socket);
    tls.write_all(b"POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello")
        .unwrap();
    tls.flush().unwrap();
    let mut response = vec![0; RESPONSE.len()];
    tls.read_exact(&mut response).unwrap();
    let alpn = tls.conn.alpn_protocol().unwrap_or_default().to_vec();
    (response, alpn)
}

fn serve_and_replay(native: Option<&str>) {
    let fixture = Fixture::new(native.unwrap_or("interpreted"));
    let tape = fixture.0.join("run.tape");
    let mut child = fixture
        .command(native)
        .arg("--grants")
        .arg(fixture.0.join("grants.json"))
        .arg("--record")
        .arg(&tape)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let Ok(port) = line.trim().parse::<u16>() else {
        let _ = child.kill();
        panic!("{native:?}: no port: {:?}", child.wait_with_output())
    };
    let (response, alpn) = fetch(port);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{native:?}: {output:?}"
    );
    assert_eq!(response, RESPONSE, "{native:?}");
    assert_eq!(alpn, b"http/1.1", "{native:?}");
    // The tape names the chain by digest; the key's base64 never appears.
    let recorded = std::fs::read_to_string(&tape).unwrap();
    let body: String = String::from_utf8(KEY.to_vec())
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect();
    assert!(!recorded.contains(&body[..32]), "{native:?}");
    assert!(recorded.contains("tls_accept"), "{native:?}");
    for replay in [None, Some("llvm")] {
        let replayed = fixture
            .command(replay)
            .arg("--replay")
            .arg(&tape)
            .output()
            .unwrap();
        assert!(
            replayed.status.success() && replayed.stderr.is_empty(),
            "{native:?} replayed by {replay:?}: {replayed:?}"
        );
        assert_eq!(replayed.stdout, line.as_bytes());
    }
}

#[test]
fn https_serves_a_verifying_client_and_replays_in_either_engine() {
    serve_and_replay(None);
    serve_and_replay(Some("llvm"));
}

#[test]
#[ignore = "full gate"]
fn full_gate_cranelift_https_serves_and_replays_in_either_engine() {
    serve_and_replay(Some("cranelift"));
}
