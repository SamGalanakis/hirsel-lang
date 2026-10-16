//! TLS runtime exports through compiled Hirsel calls, on x86-64 and AArch64/qemu.
//! rustls v/0.23.46 test-ca fixtures and API certificate/close-notify vectors;
//! the provider records plaintext and replays with the local server gone.
#[path = "../../../hirsel-driver/tests/support/source_std.rs"]
mod source_std;
use hirsel_driver::native::{
    Launched, OptLevel, Options, Partition, Sites, Toolchain, build, link, observe,
};
use hirsel_engine::{check_progress, default_context, entry_path, with_compiler_stack};
use hirsel_lir::layout_types::TargetSpec;
use hirsel_rt::io::Request;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

/// The process the driver's `launch_with` runs, plus the launcher's network grant and the
/// server port as argument 0.
fn run(
    executable: &Path,
    target: &TargetSpec,
    tools: &Toolchain,
    work: &Path,
    tape: (&str, &Path),
    rules: &str,
    port: u16,
) -> Launched {
    let root = work.join("root");
    let record = work.join("observation");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&record);
    std::fs::create_dir_all(&root).unwrap();
    let mut command = if target.triple.starts_with("aarch64") {
        let mut command = Command::new(&tools.qemu_aarch64);
        command
            .arg("-L")
            .arg(&tools.aarch64_sysroot)
            .arg(executable);
        command
    } else {
        Command::new(executable)
    };
    let output = command
        .arg(port.to_string())
        .env_clear()
        .env("HIRSEL_RT_ROOT", &root)
        .env("HIRSEL_RT_OBSERVATION", &record)
        .env("HIRSEL_RT_DIAGNOSTIC", "1")
        .env("HIRSEL_RT_NET", rules)
        .env(tape.0, tape.1)
        .output()
        .unwrap();
    Launched {
        audit_required: true,
        exit: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        record: std::fs::read_to_string(&record).unwrap_or_default(),
    }
}

/// Two rejected handshakes followed by a successful plaintext exchange.
fn serve() -> (u16, std::thread::JoinHandle<()>) {
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
    let thread = std::thread::spawn(move || {
        for rejected in [true, true, false] {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            socket
                .set_write_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let conn = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, socket);
            let mut request = [0; 4];
            if rejected {
                assert!(tls.read_exact(&mut request).is_err());
                continue;
            }
            tls.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"ping");
            tls.write_all(b"world\n").unwrap();
            tls.conn.send_close_notify();
            tls.flush().unwrap();
            let mut byte = [0];
            assert_eq!(tls.read(&mut byte).unwrap(), 0);
        }
    });
    (port, thread)
}

fn campaign(targets: Vec<TargetSpec>, opts: &'static [OptLevel], scratch_name: &'static str) {
    with_compiler_stack(move || {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join(scratch_name);
        std::fs::create_dir_all(&scratch).unwrap();
        let tools = Toolchain::in_tree_diagnostic(
            &scratch.join("runtime"),
            &targets.iter().collect::<Vec<_>>(),
        )
        .unwrap();
        let source =
            std::fs::read_to_string(root.join("native-tests/library/tls-stream.hir")).unwrap();
        let mut engine = source_std::engine();
        let context = default_context();
        let file = engine.sources.add("tls-stream.hir", source).unwrap();
        let checked = check_progress(&mut engine, file, &context).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let entry = entry_path(&context);
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let sites = Sites::new(input.snapshot());
        for target in &targets {
            let program = build(&input, &Options::new(target.clone())).unwrap();
            for &opt in opts {
                let work = scratch.join(format!("{}-{opt:?}", target.triple));
                std::fs::create_dir_all(&work).unwrap();
                let executable =
                    link(&program, &entry, opt, Partition::Whole, &tools, &work).unwrap();
                let tape = work.join("tape");
                let (port, server) = serve();
                // The root Net's io.net grant as RunConfig JSON (net-t2 launch admission).
                let rules = format!(
                    r#"{{"provider":"net.stream@1","rules":[{{"address":"127.0.0.1","service":"{port}","operation":"connect"}}]}}"#
                );
                let recorded = run(
                    &executable,
                    target,
                    &tools,
                    &work,
                    ("HIRSEL_RT_TAPE", &tape),
                    &rules,
                    port,
                );
                let observed = observe(&recorded, &program, &sites).unwrap();
                let expected = "issuer rejected\nname rejected\nworld\nclosed\n";
                assert_eq!(
                    observed.exit_code,
                    Some(0),
                    "{recorded:?}\n{}",
                    std::fs::read_to_string(&tape).unwrap_or_default()
                );
                assert!(observed.failures.is_empty(), "{observed:?}");
                assert_eq!(String::from_utf8_lossy(&observed.output), expected);
                server.join().unwrap();
                let recording =
                    hirsel_rt::launch::parse_recording(&std::fs::read_to_string(&tape).unwrap())
                        .unwrap();
                assert_eq!(
                    recording
                        .events
                        .iter()
                        .filter(|e| matches!(e.request, Request::TlsConnect { .. }))
                        .count(),
                    3
                );
                assert_eq!(
                    recording
                        .events
                        .iter()
                        .filter(|e| matches!(e.request, Request::TlsClose { .. }))
                        .count(),
                    1
                );
                assert!(recording.events.iter().any(
                    |e| matches!(&e.request, Request::NetWriteAll { bytes, .. } if bytes == b"ping")
                ));
                assert!(recording.events.iter().any(|e| matches!(&e.completion, hirsel_rt::io::Completion::NetBytes(Ok(bytes)) if bytes == b"wor")));
                // The server is gone: replay uses no socket.
                let replayed = run(
                    &executable,
                    target,
                    &tools,
                    &work,
                    ("HIRSEL_RT_REPLAY", &tape),
                    &rules,
                    port,
                );
                assert_eq!(
                    observe(&replayed, &program, &sites).unwrap(),
                    observed,
                    "{replayed:?}"
                );
                // Replay re-derives grants: without the rule, the recorded connect refuses.
                let refused = run(
                    &executable,
                    target,
                    &tools,
                    &work,
                    ("HIRSEL_RT_REPLAY", &tape),
                    "",
                    port,
                );
                assert_eq!(refused.exit, Some(3), "{refused:?}");
            }
        }
    });
}

#[test]
fn tls_exports_record_against_rustls_and_replay_plaintext_in_smoke() {
    campaign(
        vec![TargetSpec::x86_64_unknown_linux_gnu()],
        &[OptLevel::Speed],
        "tls-native-smoke",
    );
}

#[test]
#[ignore = "full gate"]
fn full_gate_tls_exports_record_and_replay_on_both_targets() {
    campaign(
        vec![
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ],
        &[OptLevel::None, OptLevel::Speed],
        "tls-native",
    );
}
