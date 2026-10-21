use super::*;

fn decode(word: &str) -> Vec<u8> {
    word.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn cores_shorthand_equals_record_placement_in_source_and_native_headers() {
    let source = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    let status = std::fs::read_to_string("/proc/thread-self/status").unwrap();
    let cpus = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
        .unwrap()
        .trim();
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let mut headers = vec![];
        for shorthand in [false, true] {
            let tape = source.with_extension(format!("{backend:?}-{shorthand}.tape"));
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run").arg(&source);
            if let Some(backend) = backend {
                command.args(["--native", "--backend", backend]);
            }
            command.arg("--grants").arg(serde_json::json!({"placement": {"cores": if shorthand {2} else {8}, "cpus": cpus}}).to_string());
            if shorthand {
                command.args(["--cores", "8"]);
            }
            let output = command.arg("--record").arg(&tape).output().unwrap();
            assert_eq!(output.status.code(), Some(0), "{backend:?}: {output:?}");
            let text = hirsel_rt::launch::read_recording_text(&tape).unwrap();
            let field = |name: &str| {
                text.lines()
                    .find_map(|line| line.strip_prefix(name))
                    .unwrap()
                    .to_owned()
            };
            let placement: serde_json::Value =
                serde_json::from_slice(&decode(&field("placement "))).unwrap();
            assert_eq!(placement["cores"], 8);
            assert_eq!(placement["cpus"], cpus);
            let canonical: serde_json::Value =
                serde_json::from_slice(&decode(&field("run "))).unwrap();
            assert!(canonical.get("placement").is_none());
            headers.push((field("run "), field("run_identity "), placement));
            // Replay ignores the placement override and never opens the provenance paths.
            let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .arg(&source)
                .arg("--replay")
                .arg(&tape)
                .args(["--cores", "1"])
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(0), "{output:?}");
            std::fs::remove_file(tape).unwrap();
        }
        assert_eq!(headers[0], headers[1]);
    }
    std::fs::remove_file(source).unwrap();
}

#[test]
fn cores_requires_one_positive_integer() {
    let source = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    for value in ["0", "-1", "auto", "1.0", "01", "18446744073709551616"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&source)
            .args(["--cores", value])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{value}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("--cores requires a positive integer")
        );
    }
    std::fs::remove_file(source).unwrap();
}

#[test]
fn cores_overrides_bound_record_and_works_without_explicit_grants() {
    let source = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    let config = source.with_extension("run.json");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["export", "--run"])
        .arg(&source)
        .arg("-o")
        .arg(&config)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    record["placement"] = serde_json::json!({"cores": 2, "run_ahead": 3});
    std::fs::write(&config, record.to_string()).unwrap();
    let tape = source.with_extension("record.tape");
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--run")
        .arg(&config)
        .args(["--cores", "8"])
        .arg("--record")
        .arg(&tape)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = hirsel_rt::launch::read_recording_text(&tape).unwrap();
    let provenance = text
        .lines()
        .find_map(|line| line.strip_prefix("placement "))
        .unwrap();
    let placement: serde_json::Value = serde_json::from_slice(&decode(provenance)).unwrap();
    assert_eq!(placement["cores"], 8);
    assert_eq!(placement["run_ahead"], 3);
    let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--cores", "8"])
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    for path in [source, config, tape] {
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn ctl_sends_a_json_line_and_reports_refusals() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    let path = file("").with_extension("ctl");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        for (expected, reply) in [
            (
                serde_json::json!({"placement":{"cores":6}}),
                "{\"width\":{\"requested\":6,\"effective\":1,\"pending\":6}}\n",
            ),
            (
                serde_json::json!({"metrics":true}),
                "{\"error\":\"refused\"}\n",
            ),
        ] {
            let (mut peer, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&peer).read_line(&mut request).unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&request).unwrap(),
                expected
            );
            peer.write_all(reply.as_bytes()).unwrap();
        }
    });
    let success = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("ctl")
        .arg(&path)
        .args(["cores", "6"])
        .output()
        .unwrap();
    assert!(success.status.success(), "{success:?}");
    assert!(String::from_utf8_lossy(&success.stdout).contains("pending"));
    let refused = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("ctl")
        .arg(&path)
        .arg("metrics")
        .output()
        .unwrap();
    assert!(!refused.status.success());
    server.join().unwrap();
    std::fs::remove_file(path).unwrap();
}

#[test]
fn native_sidecars_are_admitted_and_replay_opens_neither_path() {
    let source = file("pub fn main() -> Result[(), ()] { Ok(()) }\n");
    let metrics = source.with_extension("metrics");
    let control = source.with_extension("ctl");
    let tape = source.with_extension("tape");
    let grants =
        serde_json::json!({"placement": {"cores": 1, "metrics": metrics, "control": control}})
            .to_string();
    let live = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .args(["--native", "--grants", &grants, "--record"])
        .arg(&tape)
        .output()
        .unwrap();
    assert!(live.status.success(), "{live:?}");
    assert!(metrics.exists());
    assert!(
        !control.exists(),
        "admission must remove its socket on exit"
    );
    std::fs::write(&metrics, "metrics sentinel").unwrap();
    std::fs::write(&control, "control sentinel").unwrap();
    let replay = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .args(["--native", "--replay"])
        .arg(&tape)
        .output()
        .unwrap();
    assert!(replay.status.success(), "{replay:?}");
    assert_eq!(
        std::fs::read_to_string(&metrics).unwrap(),
        "metrics sentinel"
    );
    assert_eq!(
        std::fs::read_to_string(&control).unwrap(),
        "control sentinel"
    );
    for path in [source, metrics, control, tape] {
        std::fs::remove_file(path).unwrap();
    }
}
