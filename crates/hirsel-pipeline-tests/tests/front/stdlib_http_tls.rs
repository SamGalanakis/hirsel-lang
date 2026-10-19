//! std HTTP over a live local rustls server, using T4's rustls test-ca fixtures.
#[path = "../../../hirsel-driver/tests/support/source_std.rs"]
mod source_std;
use hirsel_driver::native::{
    Launched, OptLevel, Options, Partition, Sites, Toolchain, build, link, observe,
};
use hirsel_engine::{check_progress, default_context, entry_path, with_compiler_stack};
use hirsel_lir::layout_types::TargetSpec;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use std::{
    io::{Read, Write},
    path::Path,
    process::Command,
    sync::Arc,
};

#[test]
fn http_fetch_over_local_rustls_records_plaintext_and_replays() {
    with_compiler_stack(|| {
        let chain = CertificateDer::pem_slice_iter(include_bytes!(
            "../../../hirsel-rt/tests/it/tls-data/end.fullchain"
        ))
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        let key = PrivateKeyDer::from_pem_slice(include_bytes!(
            "../../../hirsel-rt/tests/it/tls-data/end.key"
        ))
        .unwrap();
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap(),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let roots = include_str!("../../../hirsel-rt/tests/it/tls-data/ca.cert");
        let source = format!(
            r#"
fn http_fail(take e: std@1/http.HttpError) -> () {{}}
fn grant_fail(take e: GrantError) -> () {{}}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {{
  let n = io.net(rt) ? grant_fail
  let mut config = std@1/http.default_config()
  config.roots = Pem(b{roots:?})
  config.connect_timeout = Some(std@1/duration.from_parts(5, 0))
  config.read_timeout = Some(std@1/duration.from_parts(5, 0))
  config.write_timeout = Some(std@1/duration.from_parts(5, 0))
  let response = std@1/http.fetch(n, config, Get(), "https://localhost:{port}/live", [], None()) ? http_fail
  if response.status != 200 || response.body != b"hello tls" {{ return Error(()) }}
  Ok(())
}}
"#
        );
        let mut engine = source_std::engine();
        let context = default_context();
        let file = engine.sources.add("http-tls.hir", source).unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&context);
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("std-http-tls");
        std::fs::create_dir_all(&scratch).unwrap();
        let tools = Toolchain::in_tree_diagnostic(&scratch.join("runtime"), &[&target]).unwrap();
        let program = build(&input, &Options::new(target)).unwrap();
        let sites = Sites::new(input.snapshot());
        let executable = link(
            &program,
            &entry,
            OptLevel::None,
            Partition::Whole,
            &tools,
            &scratch,
        )
        .unwrap();
        let server = std::thread::spawn(move || {
            // Nonblocking accept bounds failure when the client exits before connecting.
            // Other processes on the host may probe the loopback port, so a connection
            // that does not complete the TLS handshake is dropped and accept continues.
            listener.set_nonblocking(true).unwrap();
            let start = std::time::Instant::now();
            let mut tls = loop {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        socket.set_nonblocking(false).unwrap();
                        socket
                            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                            .unwrap();
                        socket
                            .set_write_timeout(Some(std::time::Duration::from_secs(10)))
                            .unwrap();
                        let mut conn = rustls::ServerConnection::new(config.clone()).unwrap();
                        if conn.complete_io(&mut socket).is_ok() && !conn.is_handshaking() {
                            break rustls::StreamOwned::new(conn, socket);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
                assert!(
                    start.elapsed() < std::time::Duration::from_secs(20),
                    "HTTP client never completed a TLS handshake"
                );
            };
            let mut request = vec![];
            let mut b = [0; 1];
            while !request.ends_with(b"\r\n\r\n") {
                tls.read_exact(&mut b).unwrap();
                request.push(b[0]);
                assert!(request.len() < 4096);
            }
            assert_eq!(request, format!("GET /live HTTP/1.1\r\nhost: localhost:{port}\r\nuser-agent: hirsel-std/1\r\nconnection: close\r\n\r\n").as_bytes());
            tls.write_all(b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n9\r\nhello tls\r\n0\r\nx-check: yes\r\n\r\n").unwrap();
            tls.conn.send_close_notify();
            tls.flush().unwrap();
        });
        let root = scratch.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let record = scratch.join("observation");
        let tape = scratch.join("tape");
        let rules = format!(
            r#"{{"provider":"net.stream@1","rules":[{{"address":"localhost","service":"{port}","operation":"connect_tls"}}]}}"#
        );
        let run = |mode: &str| {
            let _ = std::fs::remove_file(&record);
            let output = Command::new(&executable)
                .env_clear()
                .env("HIRSEL_RT_ROOT", &root)
                .env("HIRSEL_RT_OBSERVATION", &record)
                .env("HIRSEL_RT_DIAGNOSTIC", "1")
                .env("HIRSEL_RT_NET", &rules)
                .env(mode, &tape)
                .output()
                .unwrap();
            Launched {
                audit_required: true,
                exit: output.status.code(),
                signal: std::os::unix::process::ExitStatusExt::signal(&output.status),
                stdout: output.stdout,
                stderr: output.stderr,
                record: std::fs::read_to_string(&record).unwrap_or_default(),
            }
        };
        let recorded = run("HIRSEL_RT_TAPE");
        assert!(
            server.join().is_ok(),
            "the server failed; client exit {:?}, stderr {}",
            recorded.exit,
            String::from_utf8_lossy(&recorded.stderr)
        );
        let observed = observe(&recorded, &program, &sites).unwrap();
        assert_eq!(observed.exit_code, Some(0), "{recorded:?}");
        assert!(observed.failures.is_empty());
        let replayed = run("HIRSEL_RT_REPLAY");
        assert_eq!(
            observe(&replayed, &program, &sites).unwrap(),
            observed,
            "{replayed:?}"
        );
        let recording =
            hirsel_rt::launch::parse_recording(&std::fs::read_to_string(tape).unwrap()).unwrap();
        assert!(recording.events.iter().any(|e| matches!(&e.request, hirsel_rt::io::Request::NetWriteAll { bytes, .. } if bytes.starts_with(b"GET /live HTTP/1.1\r\n"))));
    });
}
