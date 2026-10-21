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
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let mut headers = vec![];
        for shorthand in [false, true] {
            let tape = source.with_extension(format!("{backend:?}-{shorthand}.tape"));
            let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            command.arg("run").arg(&source);
            if let Some(backend) = backend {
                command.args(["--native", "--backend", backend]);
            }
            command.args([
                "--grants",
                if shorthand {
                    r#"{"placement":{"cores":2,"cpus":"0-3"}}"#
                } else {
                    r#"{"placement":{"cores":8,"cpus":"0-3"}}"#
                },
            ]);
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
            assert_eq!(placement["cpus"], "0-3");
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
