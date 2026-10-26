use std::process::Command;
fn fixture_header() -> hirsel_rt::launch::Recording {
    hirsel_rt::launch::Recording {
        arguments: vec![],
        run_identity: [0; 32],
        hash_seed: 2,
        random_seed: 3,
        test_seed: 5,
        binding: None,
        tree: hirsel_rt::io::Tree::new(Default::default()).unwrap(),
        events: vec![],
    }
}
fn fixture_bytes(events: &[hirsel_rt::io::Event]) -> Vec<u8> {
    let mut bytes = vec![];
    let mut writer =
        hirsel_rt::launch::RecordingWriter::new(&mut bytes, &fixture_header()).unwrap();
    for event in events {
        writer.append(event).unwrap();
    }
    writer.finish().unwrap();
    drop(writer);
    bytes
}

#[test]
fn offline_view_requires_capture_identity_and_writes_logical_perfetto() {
    let root_scratch = tempfile::Builder::new()
        .prefix("hirsel-view-cli-")
        .tempdir()
        .unwrap();
    let root = root_scratch.path().to_path_buf();
    let tape = root.join("ordinary.tape");
    let out = root.join("view.json");
    use hirsel_rt::io::{Completion, Event, Request};
    let events = [
        Event {
            request: Request::Stdout { io: 1 },
            completion: Completion::Handle(Ok(2)),
            fault: None,
            resources: None,
        },
        Event {
            request: Request::Print {
                stdout: 2,
                bytes: b"hello".to_vec(),
            },
            completion: Completion::Unit(Ok(())),
            fault: None,
            resources: None,
        },
    ];
    std::fs::write(&tape, fixture_bytes(&events)).unwrap();
    let cli = env!("CARGO_BIN_EXE_hirsel");
    let refused = Command::new(cli)
        .args(["view"])
        .arg(&tape)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--capture"));
    assert!(!out.exists());
    for perfetto in [false, true] {
        let mut command = Command::new(cli);
        command
            .arg("view")
            .arg(&tape)
            .args(["--capture", "fixture", "-o"])
            .arg(&out);
        if perfetto {
            command.arg("--perfetto");
        }
        let result = command.output().unwrap();
        assert!(result.status.success(), "{result:?}");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        if perfetto {
            assert_eq!(value["hirsel"]["time_basis"], "logical-order");
            assert_eq!(value["traceEvents"][1]["ph"], "I");
        } else {
            assert_eq!(value["evidence"]["capture"], "fixture");
            assert_eq!(value["events"][0]["source"], serde_json::Value::Null);
        }
    }
    let refused = Command::new(cli)
        .arg("view")
        .arg(&tape)
        .args(["--capture", "fixture", "-o"])
        .arg(&out)
        .arg("--timing")
        .arg(root.join("missing"))
        .output()
        .unwrap();
    assert!(!refused.status.success());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn offline_view_redacts_recorded_secrets_in_json_and_perfetto() {
    let root_scratch = tempfile::Builder::new()
        .prefix("hirsel-view-secrets-")
        .tempdir()
        .unwrap();
    let root = root_scratch.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    let secret = "recorded-credential-DO-NOT-PRINT";
    let encoded = hirsel_run_schema::hex(secret.as_bytes());
    let header = hirsel_rt::launch::render_recording(&fixture_header()).unwrap();
    let header = header.split_once("events\n").unwrap().0;
    let recording = format!(
        "{header}events\nrandom_bytes 1 {} -> random ok {encoded}\nenv_get 1 544f4b454e -> env some {encoded}\nread 1 736563726574 -> bytes ok {encoded}\nnet_read 1 2 100 -> net_bytes ok {encoded}\nprint 1 {encoded} -> unit ok \ncomplete 5\n",
        secret.len()
    );
    let tape = root.join("secrets.tape");
    let output = root.join("view.json");
    let record = hirsel_rt::launch::parse_recording(&recording).unwrap();
    std::fs::write(&tape, fixture_bytes(&record.events)).unwrap();
    for perfetto in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command
            .arg("view")
            .arg(&tape)
            .args(["--capture", "fixture", "-o"])
            .arg(&output);
        if perfetto {
            command.arg("--perfetto");
        }
        let result = command.output().unwrap();
        assert!(result.status.success(), "{result:?}");
        let text = format!(
            "{}{}{}",
            std::fs::read_to_string(&output).unwrap(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!text.contains(secret));
        assert!(!text.contains(&encoded));
        assert!(text.contains(&format!("[redacted len={} sha256=", secret.len())));
        assert!(text.contains("random ok [redacted"));
        assert!(text.contains("env some [redacted"));
    }
    // --source must not leak through the replay adapter's normal stdout/stderr echo.
    let source = root.join("main.hir");
    std::fs::write(
        &source,
        format!(
            r#"
pub type AppError = Grant(GrantError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {{
  let output = io.stdout(rt) ? Grant
  let error = io.stderr(rt) ? Grant
  out.print(output, "{secret}") ? Output
  err.print(error, "{secret}") ? Output
  Ok(())
}}
"#
        ),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        // The automatic timing sidecar and the offline view name the same capture.
        .env("HIRSEL_TIMING_CAPTURE_ID", "fixture")
        .arg("run")
        .arg(&source)
        .args([
            "--grants",
            r#"{"io":{"stdout":"stdout.bytes@1","stderr":"stderr.bytes@1","dirs":[]}}"#,
            "--record",
        ])
        .arg(&tape)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert!(String::from_utf8_lossy(&result.stdout).contains(secret));
    assert!(String::from_utf8_lossy(&result.stderr).contains(secret));
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--replay")
        .arg(&tape)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert!(String::from_utf8_lossy(&result.stdout).contains(secret));
    assert!(String::from_utf8_lossy(&result.stderr).contains(secret));
    let result = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("view")
        .arg(&tape)
        .args(["--capture", "fixture", "-o"])
        .arg(&output)
        .arg("--source")
        .arg(&source)
        .args(["--analysis", "analysis"])
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert!(
        result.stdout.is_empty(),
        "replayed stdout must remain private"
    );
    assert!(
        result.stderr.is_empty(),
        "replayed stderr must remain private"
    );
    let text = std::fs::read_to_string(&output).unwrap();
    assert!(!text.contains(secret));
    assert!(!text.contains(&encoded));
    std::fs::remove_dir_all(root).unwrap();
}
