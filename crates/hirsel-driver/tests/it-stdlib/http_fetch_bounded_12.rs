//! Go net/http request.go:1222-1254 and serve_test.go:3660-3680 at ebe2feef:
//! exact/over-limit vectors, adapted to typed Hirsel errors and offline replay.
//! Source/license/adaptations: fixtures/http-fetch-bounded-12/REFERENCES.md.
use hirsel_driver::*;
use hirsel_engine::*;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/http-fetch-bounded-12/{name}.hir"))
}
fn checked(path: &Path) -> LoadedProgram {
    let mut loaded = load_program(path).unwrap();
    loaded.check_for_execution().unwrap_or_else(|error| {
        if let ToolError::Query {
            error: hirsel_types::QueryError::Diagnostics(diagnostics),
            sources,
            ..
        } = &error
        {
            for diagnostic in diagnostics {
                eprintln!("{}", diagnostic.render_human(sources));
            }
        }
        panic!("{error}")
    });
    loaded
}
fn ready(loaded: &mut LoadedProgram) -> native::ReadyProgram {
    let input = loaded.engine.export_native_input().unwrap();
    let validated = loaded.engine.import_native_input(input).unwrap();
    native::build(
        &validated,
        &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
    )
    .unwrap()
}
fn ok(outcome: RunOutcome) {
    assert!(
        matches!(
            outcome,
            RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
        ),
        "{outcome:?}"
    );
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn scripted_error_precedence_body_send_and_resource_replay() {
    with_compiler_stack(|| {
        let path = fixture("scripted");
        let mut loaded = checked(&path);
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let recorded =
            run_observed_snapshot(&mut loaded.engine, &config, SimulationConfig::default())
                .unwrap();
        ok(recorded.outcome.unwrap());
        let observation = recorded.observation.unwrap();
        let checked =
            run_observed_checked(&mut loaded.engine, &config, SimulationConfig::default()).unwrap();
        assert_eq!(checked.observation.as_ref(), Some(&observation));
        let events: Vec<_> = observation
            .effects
            .iter()
            .filter_map(|e| match &e.effect {
                hirsel_interp::observation::Effect::CapabilityConsumed(event) => {
                    Some(event.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            events.iter().filter(|e| e.operation == "net.close").count(),
            7
        );
        let replayed = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                inputs: events,
                ..config
            },
            SimulationConfig::default(),
        )
        .unwrap();
        ok(replayed.outcome.unwrap());
        assert_eq!(replayed.observation.as_ref(), Some(&observation));
        let program = ready(&mut loaded);
        let result = native::run_heap(
            &program,
            &loaded.entry,
            &native::Sites::new(&loaded.engine.export_native_input().unwrap()),
        )
        .unwrap();
        assert_eq!(result.observation.exit_code, Some(0));
        assert_eq!(result.counters.live_heap_bytes, 0);
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            ok(native::run_path_native_backend_with_validation(
                &path,
                HostLaunch::empty(),
                None,
                backend,
                native::Output::Streams {
                    stdout: &mut Vec::new(),
                    stderr: &mut Vec::new(),
                },
                Verify::On,
                native::Validation::EveryStage,
            )
            .unwrap()
            .unwrap());
        }
    });
}

struct Case {
    name: String,
    wire: Vec<u8>,
    limit: u64,
    config: u64,
    coding: &'static str,
    status_error: bool,
    api: &'static str,
    expected: String,
    redirect: bool,
    native_only: bool,
}
impl Case {
    fn new(name: &str, wire: Vec<u8>, limit: u64, expected: &str) -> Self {
        Self {
            name: name.into(),
            wire,
            limit,
            config: 10485760,
            coding: "off",
            status_error: true,
            api: "bounded",
            expected: expected.into(),
            redirect: false,
            native_only: false,
        }
    }
}
fn length(body: &[u8], status: u16, extra: &str) -> Vec<u8> {
    let mut wire = format!(
        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\n{extra}\r\n",
        body.len()
    )
    .into_bytes();
    wire.extend(body);
    wire
}
fn success(body: &[u8], status: u16, encoded: bool) -> String {
    format!(
        "ok {status} {} {} {} {{url}}\n",
        body.len(),
        if body.len() <= 4097 {
            hex(body)
        } else {
            "all-x".into()
        },
        if encoded { "encoded" } else { "plain" }
    )
}
fn cases() -> Vec<Case> {
    let mut rows = vec![];
    for framing in ["length", "chunked", "eof"] {
        for body in [&b""[..], &b"a"[..], &b"\0\xffxyz"[..]] {
            for limit in [0, body.len() as u64, body.len() as u64 + 1, u64::MAX] {
                let wire = match framing {
                    "length" => length(body, 200, ""),
                    "chunked" => {
                        let mut wire =
                            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
                        if !body.is_empty() {
                            wire.extend(format!("{:x};a=b\r\n", body.len()).as_bytes());
                            wire.extend(body);
                            wire.extend(b"\r\n");
                        }
                        wire.extend(b"0\r\nX-Trailer: yes\r\n\r\n");
                        wire
                    }
                    _ => {
                        let mut wire = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
                        wire.extend(body);
                        wire
                    }
                };
                let expected = if body.len() as u64 <= limit {
                    success(body, 200, false)
                } else {
                    "BodyTooLarge\n".into()
                };
                rows.push(Case::new(
                    &format!("{framing}-{}-{limit}", body.len()),
                    wire,
                    limit,
                    &expected,
                ));
            }
        }
    }
    for (explicit, config) in [(5, 4), (4, 5), (5, 5)] {
        let mut row = Case::new(
            "two-limits",
            length(b"abcde", 200, ""),
            explicit,
            if explicit.min(config) < 5 {
                "BodyTooLarge\n"
            } else {
                "ok 200 5 6162636465 plain {url}\n"
            },
        );
        row.config = config;
        rows.push(row);
    }
    for (name, wire, limit, expected) in [
        (
            "truncated-length",
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nab".as_slice(),
            2,
            "Truncated\n",
        ),
        (
            "truncated-chunk",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nab",
            2,
            "Truncated\n",
        ),
        (
            "malformed-chunk",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nq\r\n",
            0,
            "Chunk\n",
        ),
        (
            "framing-before-status",
            b"HTTP/1.1 500 Error\r\nContent-Length: no\r\n\r\n",
            0,
            "ContentLength\n",
        ),
        (
            "status-before-body",
            b"HTTP/1.1 500 Error\r\nContent-Length: 3\r\nContent-Encoding: br\r\n\r\n",
            0,
            "Status 500\n",
        ),
        (
            "unsupported",
            b"HTTP/1.1 200 OK\r\nContent-Encoding: br\r\n\r\nab",
            2,
            "UnsupportedContentEncoding\n",
        ),
        (
            "raw-before-coding",
            b"HTTP/1.1 200 OK\r\nContent-Encoding: br\r\n\r\nab",
            1,
            "BodyTooLarge\n",
        ),
    ] {
        let mut row = Case::new(name, wire.into(), limit, expected);
        row.coding = "auto";
        rows.push(row);
    }
    let mut status = Case::new(
        "status-disabled",
        length(b"err", 500, ""),
        3,
        &success(b"err", 500, false),
    );
    status.status_error = false;
    rows.push(status);
    for limit in [1, 2] {
        let mut row = Case::new(
            "redirect-final",
            length(b"ok", 200, ""),
            limit,
            if limit == 1 {
                "BodyTooLarge\n"
            } else {
                "ok 200 2 6f6b plain {url}/next\n"
            },
        );
        row.redirect = true;
        rows.push(row);
    }
    // Independent tiny compressed vectors: 200 'a' bytes (Go-style decode bound).
    let gzip = include_bytes!("../fixtures/http-fetch-bounded-12/a200.gz");
    let deflate = include_bytes!("../fixtures/http-fetch-bounded-12/a200.zlib");
    for (coding, compressed) in [("gzip", gzip.as_slice()), ("deflate", deflate.as_slice())] {
        for limit in [compressed.len() as u64 - 1, 100, 199, 200] {
            let expected = if limit < 200 {
                "BodyTooLarge\n".into()
            } else {
                success(&[b'a'; 200], 200, false)
            };
            let mut row = Case::new(
                coding,
                length(compressed, 200, &format!("Content-Encoding: {coding}\r\n")),
                limit,
                &expected,
            );
            row.coding = "auto";
            // Config bounds raw bytes only, even when decoded output is larger.
            row.config = compressed.len() as u64;
            rows.push(row);
        }
        for mode in ["caller", "off"] {
            let mut row = Case::new(
                "decoding-disabled",
                length(compressed, 200, &format!("Content-Encoding: {coding}\r\n")),
                compressed.len() as u64,
                &success(compressed, 200, true),
            );
            row.coding = mode;
            rows.push(row);
        }
    }
    let mut corrupt = gzip.to_vec();
    let last = corrupt.len() - 8;
    corrupt[last] ^= 1;
    let mut row = Case::new(
        "checksum",
        length(&corrupt, 200, "Content-Encoding: gzip\r\n"),
        200,
        "Checksum\n",
    );
    row.coding = "auto";
    rows.push(row);
    for size in [4095, 4096, 4097] {
        rows.push(Case::new(
            "read-boundary",
            length(&vec![b'x'; size], 200, ""),
            size as u64,
            &success(&vec![b'x'; size], 200, false),
        ));
    }
    for (api, size, limit, config) in [
        ("legacy", 2, 0, 0),
        ("legacy", 10485760, 0, 20000000),
        ("legacy", 10485761, 0, 20000000),
        ("bounded", 10485761, 10485761, 20000000),
    ] {
        let body = vec![b'x'; size];
        let expected = if api == "legacy" && size > 10485760 {
            "BodyTooLarge\n".into()
        } else {
            success(&body, 200, false)
        };
        let mut row = Case::new("legacy-10MiB", length(&body, 200, ""), limit, &expected);
        row.api = api;
        row.config = config;
        row.native_only = size > 4097;
        rows.push(row);
    }
    rows
}

fn read_tape(path: &Path) -> hirsel_rt::launch::Recording {
    hirsel_rt::launch::parse_recording(&hirsel_rt::launch::read_recording_text(path).unwrap())
        .unwrap()
}
fn prints(events: &[hirsel_rt::io::Event]) -> Vec<u8> {
    events
        .iter()
        .flat_map(|event| match &event.request {
            hirsel_rt::io::Request::Print { bytes, .. } => bytes.clone(),
            _ => vec![],
        })
        .collect()
}
fn assert_closed(events: &[hirsel_rt::io::Event], count: usize) {
    use hirsel_rt::io::{Completion, Request};
    // A parked connect is requested again after readiness; count successful
    // handles rather than request attempts, and close each handle exactly once.
    let handles: Vec<_> = events
        .iter()
        .filter_map(|event| match (&event.request, &event.completion) {
            (Request::Connect { .. }, Completion::NetHandle(Ok(handle))) => Some(*handle),
            _ => None,
        })
        .collect();
    assert_eq!(handles.len(), count);
    for handle in handles {
        assert_eq!(events.iter().filter(|event| matches!(event.request, Request::Close { handle: closed } if closed == handle)).count(), 1,
            "every connected stream must close once on success and error");
    }
}

#[test]
fn loopback_boundaries_decoding_legacy_and_offline_replay() {
    with_compiler_stack(|| {
        let path = fixture("main");
        let mut loaded = checked(&path);
        let program = ready(&mut loaded);
        let retain = std::env::var_os("HTTP_FETCH_BOUNDED_12_EVIDENCE");
        let base = retain
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
        let dir = base.join(format!("http-fetch-bounded-12-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let executables = if let Some(prebuilt) = std::env::var_os("HTTP_FETCH_BOUNDED_12_PREBUILT")
        {
            ["Llvm", "Cranelift"]
                .iter()
                .map(|backend| PathBuf::from(&prebuilt).join(backend).join("program"))
                .collect::<Vec<_>>()
        } else {
            let target = native::TargetSpec::x86_64_unknown_linux_gnu();
            let tools = native::Toolchain::in_tree_with_runtime(
                &dir.join("runtime"),
                &[&target],
                native::RuntimeBuild::default(),
            )
            .unwrap();
            let mut executables = vec![];
            // Freeze one executable per backend and reuse it across launch inputs.
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                let out = dir.join(format!("{backend:?}"));
                std::fs::create_dir_all(&out).unwrap();
                let objects = native::emit_backend_objects(
                    &program,
                    backend,
                    native::OptLevel::Speed,
                    native::Partition::Whole,
                    false,
                    None,
                )
                .unwrap();
                executables.push(
                    native::link_standalone_objects(
                        &program,
                        &loaded.entry,
                        &objects,
                        &tools,
                        &out,
                    )
                    .unwrap(),
                );
            }
            executables
        };
        let mut rows = cases();
        rows.push(Case::new("read-timeout", vec![], 0, "TimedOut\n"));
        rows.push(Case::new("denied", vec![], 0, "NotGranted\n"));
        let mut recordings = vec![];
        let mut launches = 0;
        for (row_index, row) in rows.iter().enumerate() {
            for engine in 0..3 {
                if row.native_only && engine == 0 {
                    continue;
                }
                // Resume an interrupted matrix only when its completed observation
                // and per-stream cleanup assertions already passed unchanged inputs.
                let prior = std::env::var_os("HTTP_FETCH_BOUNDED_12_RESUME").and_then(|previous| {
                    std::env::split_paths(&previous)
                        .map(|root| root.join(format!("{row_index}-{}-{engine}.tape", row.name)))
                        .find(|tape| tape.is_file())
                });
                if let Some(tape) = prior {
                    let record = read_tape(&tape);
                    let expected = row.expected.replace("{url}", &record.arguments[0]);
                    if prints(&record.events) == expected.as_bytes() {
                        if row.name != "denied" {
                            assert_closed(&record.events, if row.redirect { 2 } else { 1 });
                        }
                        eprintln!("reuse proved row {row_index} {} engine {engine}", row.name);
                        recordings.push((tape, expected, row.native_only));
                        launches += 1;
                        continue;
                    }
                }
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let port = listener.local_addr().unwrap().port();
                let target = format!("/bounded-{}-{row_index}-{engine}", std::process::id());
                let url = format!("http://127.0.0.1:{port}{target}");
                let expected = row.expected.replace("{url}", &url);
                let denied = row.name == "denied";
                let peer = if denied {
                    drop(listener);
                    None
                } else {
                    let wire = row.wire.clone();
                    let redirect = row.redirect;
                    let coding = row.coding;
                    let label = format!("row {row_index} {} engine {engine}", row.name);
                    let wire_path = dir.join(format!("{row_index}-{}-{engine}.request", row.name));
                    Some(std::thread::spawn(move || {
                        listener.set_nonblocking(true).unwrap();
                        for exchange in 0..if redirect { 2 } else { 1 } {
                            // Bounded accept avoids a hung helper when a caller fails before connect.
                            let deadline = std::time::Instant::now() + Duration::from_secs(30);
                            let (mut stream, request) = loop {
                                let mut stream = loop {
                                    match listener.accept() {
                                        Ok((stream, _)) => break stream,
                                        Err(error)
                                            if error.kind() == std::io::ErrorKind::WouldBlock
                                                && std::time::Instant::now() < deadline =>
                                        {
                                            std::thread::sleep(Duration::from_millis(5))
                                        }
                                        Err(error) => panic!("accept: {error}"),
                                    }
                                };
                                stream
                                    .set_read_timeout(Some(Duration::from_secs(10)))
                                    .unwrap();
                                stream
                                    .set_write_timeout(Some(Duration::from_secs(10)))
                                    .unwrap();
                                let mut request = vec![];
                                while !request.ends_with(b"\r\n\r\n") {
                                    let mut byte = [0];
                                    assert_eq!(stream.read(&mut byte).unwrap(), 1);
                                    request.push(byte[0]);
                                    assert!(request.len() < 8192);
                                }
                                let request = String::from_utf8(request).unwrap();
                                let expected_target = if exchange == 1 {
                                    format!("{target}/next")
                                } else {
                                    target.clone()
                                };
                                if !request
                                    .starts_with(&format!("GET {expected_target} HTTP/1.1\r\n"))
                                {
                                    // Shared-host HTTP probes can discover ephemeral listeners.
                                    // They do not belong to this uniquely addressed exchange.
                                    eprintln!("{label}: foreign request={request:?}");
                                    std::fs::write(
                                        wire_path.with_extension("foreign-request"),
                                        request.as_bytes(),
                                    )
                                    .unwrap();
                                    let _ = stream.shutdown(Shutdown::Both);
                                    continue;
                                }
                                std::fs::write(&wire_path, request.as_bytes()).unwrap();
                                break (stream, request);
                            };
                            assert!(
                                request.contains("connection: close\r\n"),
                                "{label}, exchange {exchange}: request={request:?}"
                            );
                            match coding {
                                "auto" => {
                                    assert!(request.contains("accept-encoding: gzip, deflate\r\n"))
                                }
                                "caller" => {
                                    assert!(request.contains("Accept-Encoding: gzip\r\n"));
                                    assert!(!request.contains("accept-encoding: gzip, deflate"));
                                }
                                _ => assert!(
                                    !request.to_ascii_lowercase().contains("accept-encoding:")
                                ),
                            }
                            if redirect && exchange == 0 {
                                // Redirect body is not collected; only the final response is bounded.
                                let _ = stream.write_all(format!("HTTP/1.1 302 Found\r\nLocation: {target}/next\r\nContent-Length: 999\r\n\r\n").as_bytes());
                            } else if !wire.is_empty() {
                                // Early status/size errors may close before the peer finishes writing.
                                let _ = stream.write_all(&wire);
                            }
                            if !wire.is_empty() {
                                let _ = stream.shutdown(Shutdown::Write);
                            }
                            let mut byte = [0];
                            match stream.read(&mut byte) {
                                Ok(0) => (),
                                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => (),
                                other => panic!(
                                    "caller did not release its one-shot connection: {other:?}"
                                ),
                            }
                        }
                    }))
                };
                let grants = serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[],"net":{"provider":"net.stream@1","rules": if denied { vec![] } else { vec![serde_json::json!({"address":"127.0.0.1","service":port.to_string(),"operation":"connect"})] }}},"args":[]}).to_string();
                let args: Vec<std::ffi::OsString> = [
                    url,
                    row.limit.to_string(),
                    row.config.to_string(),
                    row.coding.into(),
                    if row.status_error {
                        "error".into()
                    } else {
                        "allow".into()
                    },
                    row.api.into(),
                ]
                .into_iter()
                .map(Into::into)
                .collect();
                let tape = dir.join(format!("{row_index}-{}-{engine}.tape", row.name));
                eprintln!("loopback row {row_index} {} engine {engine}", row.name);
                if engine == 0 {
                    ok(run_path_on_host(
                        &path,
                        HostLaunch::Live {
                            config: LaunchConfig::Grants {
                                input: std::ffi::OsStr::new(&grants),
                                arguments: Some(&args),
                            },
                            record: Some(&tape),
                            revocations: None,
                        },
                        Some(20_000_000),
                        Verify::On,
                    )
                    .unwrap());
                } else {
                    let mut command = Command::new(&executables[engine - 1]);
                    if !row.native_only {
                        command.arg("--hirsel-record").arg(&tape);
                    }
                    let output = command
                        .arg("--hirsel-grants")
                        .arg(&grants)
                        .args(&args)
                        .output()
                        .unwrap();
                    assert!(
                        output.status.success(),
                        "{} engine {engine}: {output:?}",
                        row.name
                    );
                    assert_eq!(
                        output.stdout,
                        expected.as_bytes(),
                        "{} engine {engine}",
                        row.name
                    );
                    assert!(output.stderr.is_empty(), "{output:?}");
                }
                if let Some(peer) = peer {
                    peer.join().unwrap();
                }
                if row.native_only {
                    launches += 1;
                    continue;
                }
                let record = read_tape(&tape);
                assert_eq!(
                    prints(&record.events),
                    expected.as_bytes(),
                    "{} engine {engine}",
                    row.name
                );
                if !denied {
                    assert_closed(&record.events, if row.redirect { 2 } else { 1 });
                }
                // Listeners, peer threads and live grants are gone before replay.
                recordings.push((tape, expected, row.native_only));
                launches += 1;
            }
        }
        let mut replays = 0;
        for (tape, expected, native_only) in &recordings {
            if !native_only {
                let observation = replay_observations(&path, tape, Some(20_000_000)).unwrap();
                assert_eq!(observation.output, expected.as_bytes());
                assert_eq!(observation.exit_code, Some(0));
                assert!(observation.failures.is_empty());
                replays += 1;
            }
            for executable in &executables {
                let output = Command::new(executable)
                    .arg("--hirsel-replay")
                    .arg(tape)
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{tape:?}: {output:?}");
                assert_eq!(output.stdout, expected.as_bytes(), "{tape:?}");
                assert!(output.stderr.is_empty(), "{output:?}");
                replays += 1;
            }
        }
        eprintln!(
            "{} loopback rows; {launches} live launches; {} recordings; {replays} offline replays; builds per backend={}",
            rows.len(),
            recordings.len(),
            if std::env::var_os("HTTP_FETCH_BOUNDED_12_PREBUILT").is_some() {
                0
            } else {
                1
            }
        );
        if retain.is_none() {
            std::fs::remove_dir_all(&dir).unwrap();
        } else {
            eprintln!("retained: {}", dir.display());
        }
    });
}
