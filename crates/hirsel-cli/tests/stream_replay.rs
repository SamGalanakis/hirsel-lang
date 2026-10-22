//! Native recordings of owned stream pipelines are executable interpreter inputs.
//! Adapted from cli/replay.rs (host removal) and cli/tape_identity.rs (event order).
hirsel_guard::install!();

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

const FILE: &str = include_str!("stream_replay/file.hir");
const HTTP: &str = include_str!("stream_replay/http.hir");
const NET: &str = include_str!("stream_replay/net.hir");
const CANCEL: &str = include_str!("../../../conformance/CONC/CONC-3-stream-scoped-cancel-run.hir");
const BACKENDS: &[&str] = &["llvm", "cranelift"];

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str, source: &str) -> Self {
        let root_scratch = tempfile::Builder::new()
            .prefix(&format!("hirsel-stream-replay-{name}-"))
            .tempdir()
            .unwrap();
        let root = root_scratch.path().to_path_buf();
        std::fs::create_dir_all(root.join("data")).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(root.join("main.hir"), source).unwrap();
        std::fs::write(root.join("main.hir.json"), serde_json::json!({
            "schema": 1, "package": {"name": "stream_replay", "version": "1"},
            "sources": [{"path": "main.hir", "module": []}],
            "dependencies": [{"manifest": stdlib, "package": {"name": "std", "version": "1"}, "expose": []}],
            "entry": {"module": [], "name": "main"},
        }).to_string()).unwrap();
        let _ = root_scratch.keep();
        Self(root)
    }

    fn run(&self, backend: Option<&str>, flag: &str, tape: &Path) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .arg("run")
            .arg(self.0.join("main.hir.json"))
            .arg(flag)
            .arg(tape)
            .env("XDG_CACHE_HOME", self.0.join("cache"))
            .stdin(Stdio::null());
        if flag == "--record" {
            command.arg("--grants").arg(self.0.join("grants.json"));
        }
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        }
        command.output().unwrap()
    }

    fn grants(&self, value: serde_json::Value) {
        std::fs::write(self.0.join("grants.json"), value.to_string()).unwrap();
    }

    fn record(&self, backend: &str, expected: &[u8]) -> (PathBuf, String) {
        let tape = self.0.join(format!("{backend}.tape"));
        assert_output(&self.run(Some(backend), "--record", &tape), expected);
        let text = hirsel_rt::launch::read_recording_text(&tape).unwrap();
        assert!(text.contains("\nevents\n"), "{text}");
        (tape, text)
    }

    fn replay(&self, tape: &Path, expected: &[u8]) {
        // No grants, stdin, host directory, socket server or live clock provider
        // is supplied to the interpreter. Its replay library matches every
        // request and consumes the entire native tape, including close/schedule.
        assert!(!self.0.join("grants.json").exists());
        assert_output(&self.run(None, "--replay", tape), expected);
    }

    fn edited_refuses(&self, tape: &Path, original: &str, from: &str, to: &str) {
        assert!(original.contains(from));
        std::fs::write(self.0.join("main.hir"), original.replacen(from, to, 1)).unwrap();
        let output = self.run(None, "--replay", tape);
        std::fs::write(self.0.join("main.hir"), original).unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains(r#"replay refused: {"kind":"binding","field":"program"}"#),
            "{output:?}"
        );
    }

    fn remove_grants(&self) {
        std::fs::remove_file(self.0.join("grants.json")).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_output(output: &Output, expected: &[u8]) {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(output.stdout, expected);
}

fn events(text: &str) -> &str {
    text.split_once("\nevents\n").unwrap().1
}

#[test]
fn file_pull_chunks_prefix_zero_and_sink_error_replay_without_files() {
    let fixture = Fixture::new("file", FILE);
    std::fs::write(fixture.0.join("data/input"), b"a\xc3\xa9z").unwrap();
    fixture.grants(serde_json::json!({"io": {
        "stdout": "stdout.bytes@1", "dirs": [{"path": "data", "host": fixture.0.join("data"), "access": "read"}],
    }}));
    let expected = b"map 1\nfilter\nmap 1\nfilter\nmap 1\nfilter\nmap 1\nfilter\ncomplete\nzero\nprefix\nwrite 1\ndestination dropped\nerror returned\n";
    let recordings: Vec<_> = BACKENDS
        .iter()
        .map(|backend| fixture.record(backend, expected))
        .collect();
    // Independent native backends must preserve the exact effect and pull order.
    assert_eq!(events(&recordings[0].1), events(&recordings[1].1));
    for (_, text) in &recordings {
        let reads: Vec<_> = events(text)
            .lines()
            .filter(|line| line.starts_with("file_read "))
            .collect();
        assert_eq!(
            reads.len(),
            7,
            "four bytes + EOF, zero, prefix, writer error: {text}"
        );
        for (line, bytes) in reads.iter().zip(["61", "c3", "a9", "7a", "", "61", "61"]) {
            assert!(line.contains(" 1 -> bytes ok "), "{line}");
            assert_eq!(line.split_once(" -> bytes ok ").unwrap().1, bytes);
        }
        let files: Vec<_> = events(text)
            .lines()
            .filter(|line| line.starts_with("open "))
            .map(|line| line.split_once(" -> handle ok ").unwrap().1)
            .collect();
        assert_eq!(files.len(), 4);
        for (file, count) in files.iter().zip([5, 0, 1, 1]) {
            assert_eq!(
                reads
                    .iter()
                    .filter(|line| line.split_whitespace().nth(2) == Some(*file))
                    .count(),
                count,
                "file {file}: {text}"
            );
        }
        // Close occurs before the next stage can print that consumption returned.
        assert_eq!(
            events(text)
                .lines()
                .filter(|line| line.starts_with("close "))
                .count(),
            4,
            "{text}"
        );
    }
    std::fs::remove_dir_all(fixture.0.join("data")).unwrap();
    fixture.remove_grants();
    for (tape, _) in &recordings {
        fixture.replay(tape, expected);
        fixture.edited_refuses(tape, FILE, "stream.chunk_limit(1)", "stream.chunk_limit(2)");
    }
}

/// Wait for this fixture's client, or stop when its command has failed.
fn accept(listener: &TcpListener, stop: &std::sync::mpsc::Receiver<()>) -> Option<TcpStream> {
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                match stop.recv_timeout(Duration::from_millis(10)) {
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    _ => return None,
                }
            }
            Err(e) => panic!("accept: {e}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    Some(stream)
}

/// A real loopback server; compilation/source failure signals immediate shutdown.
fn server(
    listener: TcpListener,
    stop: std::sync::mpsc::Receiver<()>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let responses: &[&[u8]] = &[
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\na\r\n2\r\n\xc3\xa9\r\n1\r\nz\r\n0\r\nX-End: yes\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nskip\n\"\xc3\xa9\"\r\nlast\n\"unfinished",
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nx",
        ];
        listener.set_nonblocking(true).unwrap();
        for response in responses {
            let Some(mut stream) = accept(&listener, &stop) else {
                return;
            };
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 4096);
            }
            stream.write_all(response).unwrap();
        }
    })
}

#[test]
fn http_chunks_scoped_csv_clock_schedule_and_truncation_replay_without_server() {
    let expected = "byte 1\nbyte 1\nbyte 1\nbyte 1\nchunks done\nrow é\nrow last\nbody dropped\nscoped returned\ntruncated once\nall closed\n".as_bytes();
    for backend in BACKENDS {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let source = HTTP.replace("PORT", &port.to_string());
        let fixture = Fixture::new(&format!("http-{backend}"), &source);
        fixture.grants(serde_json::json!({
            "inputs": {"hash_seed": 0, "random_seed": 0, "test_seed": 0,
                "scheduler": {"algorithm": "random", "version": 1, "seed": 1}},
            "io": {"stdout": "stdout.bytes@1", "tasks": "tasks.cooperative@1",
                "monotonic_clock": "clock.monotonic.ns@1", "dirs": [],
                "net": {"provider": "net.stream@1", "rules": [
                    {"address": "127.0.0.1", "service": port.to_string(), "operation": "connect"},
                ]}},
        }));
        let (stop, stopped) = std::sync::mpsc::channel();
        let serving = server(listener, stopped);
        let tape = fixture.0.join("http.tape");
        let output = fixture.run(Some(backend), "--record", &tape);
        let _ = stop.send(());
        serving.join().unwrap();
        assert_output(&output, expected);
        let text = hirsel_rt::launch::read_recording_text(&tape).unwrap();
        for request in [
            "net_read ",
            "net_write_all ",
            "close ",
            "monotonic ",
            "sleep_until ",
            "schedule ",
        ] {
            assert!(
                events(&text).lines().any(|line| line.starts_with(request)),
                "missing {request}: {text}"
            );
        }
        assert_eq!(
            events(&text)
                .lines()
                .filter(|line| line.starts_with("close "))
                .count(),
            3,
            "{text}"
        );
        fixture.remove_grants();
        fixture.replay(&tape, expected);
        fixture.edited_refuses(&tape, &source, "refill_bytes: 1", "refill_bytes: 2");
        fixture.edited_refuses(
            &tape,
            &source,
            "let _ = task.check_canceled(child)",
            "let _ = task.check_canceled(child)\nlet _ = task.check_canceled(child)",
        );
    }
}

#[test]
fn rejecting_ready_csv_source_cancellation_and_finalizers_replay() {
    let fixture = Fixture::new("cancel", CANCEL);
    fixture.grants(serde_json::json!({"io": {
        "stdout": "stdout.bytes@1", "tasks": "tasks.cooperative@1", "dirs": [],
    }}));
    let expected = b"source 16\ncanceled\nsource 16\ncanceled\n";
    let recordings: Vec<_> = BACKENDS
        .iter()
        .map(|backend| fixture.record(backend, expected))
        .collect();
    assert_eq!(events(&recordings[0].1), events(&recordings[1].1));
    fixture.remove_grants();
    for (tape, _) in &recordings {
        fixture.replay(tape, expected);
    }
}

#[test]
fn net_chunks_and_partial_writer_suffixes_replay_without_peer() {
    let expected = b"read closed\nremaining 4\nremaining 3\nremaining 2\nremaining 1\nwriter dropped\nwrite returned\n";
    for backend in BACKENDS {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let source = NET.replace("PORT", &port.to_string());
        let fixture = Fixture::new(&format!("net-{backend}"), &source);
        fixture.grants(serde_json::json!({"io": {
            "stdout": "stdout.bytes@1", "dirs": [],
            "net": {"provider": "net.stream@1", "rules": [
                {"address": "127.0.0.1", "service": port.to_string(), "operation": "connect"},
            ]},
        }}));
        // The listener is nonblocking only so compilation/source failure can
        // signal shutdown rather than leave a server waiting for a connection.
        listener.set_nonblocking(true).unwrap();
        let (stop, stopped) = std::sync::mpsc::channel();
        let serving = std::thread::spawn(move || {
            for sending in [true, false] {
                let Some(mut stream) = accept(&listener, &stopped) else {
                    return;
                };
                if sending {
                    stream.write_all(b"a\xc3\xa9z").unwrap();
                } else {
                    let mut received = Vec::new();
                    stream.read_to_end(&mut received).unwrap();
                    assert_eq!(received, b"a\xc3\xa9z");
                }
            }
        });
        let tape = fixture.0.join("net.tape");
        let output = fixture.run(Some(backend), "--record", &tape);
        let _ = stop.send(());
        serving.join().unwrap();
        assert_output(&output, expected);
        let text = hirsel_rt::launch::read_recording_text(&tape).unwrap();
        let writes: Vec<_> = events(&text)
            .lines()
            .filter(|line| line.starts_with("net_write "))
            .collect();
        assert_eq!(writes.len(), 4, "{text}");
        for (line, byte) in writes.iter().zip(["61", "c3", "a9", "7a"]) {
            assert!(line.contains(&format!(" {byte} -> count ok 1")), "{line}");
        }
        assert_eq!(
            events(&text)
                .lines()
                .filter(|line| line.starts_with("net_read ") && !line.contains(" -> blocked"))
                .count(),
            5,
            "{text}"
        );
        assert_eq!(
            events(&text)
                .lines()
                .filter(|line| line.starts_with("close "))
                .count(),
            2,
            "{text}"
        );
        fixture.remove_grants();
        fixture.replay(&tape, expected);
    }
}

#[test]
fn http2_real_accepted_connection_records_and_replays_on_both_backends() {
    let source = include_str!("stream_replay/http2_accept.hir");
    for backend in BACKENDS {
        let reservation = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        let fixture = Fixture::new(
            &format!("h2-accepted-{backend}"),
            &source.replace("PORT", &port.to_string()),
        );
        fixture.grants(serde_json::json!({
            "io": {"stdout": "stdout.bytes@1", "tasks": "tasks.cooperative@1", "dirs": [],
                "net": {"provider": "net.stream@1", "rules": [
                    {"address": "127.0.0.1", "service": port.to_string(), "operation": "listen"},
                    {"address": "127.0.0.1", "service": port.to_string(), "operation": "connect"}
                ]}}
        }));
        drop(reservation);
        let (tape, text) = fixture.record(backend, b"h2 accepted\n");
        for operation in [
            "listen ",
            "accept ",
            "connect_addr ",
            "net_write_all ",
            "net_read ",
            "close ",
        ] {
            assert!(
                events(&text)
                    .lines()
                    .any(|line| line.starts_with(operation)),
                "missing {operation}: {text}"
            );
        }
        fixture.remove_grants();
        fixture.replay(&tape, b"h2 accepted\n");
        for replay_backend in BACKENDS {
            assert_output(
                &fixture.run(Some(replay_backend), "--replay", &tape),
                b"h2 accepted\n",
            );
        }
    }
}
