use std::path::Path;
use std::process::{Command, Output};

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command
        .env("HIRSEL_DAEMON", "off")
        .env("HIRSEL_RESULTS", "none@1")
        .env("HIRSEL_JOBS", "1");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("HIRSEL_RT_") {
            command.env_remove(key);
        }
    }
    command
}
fn success(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"{\"count\":5,\"sum\":7,\"min\":-9,\"max\":12}\n"
    );
}
pub fn file_and_replay() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let data = root.join("data");
    std::fs::create_dir(&data).unwrap();
    let input = include_bytes!("../../../../../examples/csvreduce/fixtures/mixed.csv");
    std::fs::write(data.join("input.csv"), input).unwrap();
    std::fs::write(
        root.join("main.hir"),
        include_str!("../../../../../examples/csvreduce/main.hir"),
    )
    .unwrap();
    let mut manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../examples/csvreduce/hirsel.json"
    ))
    .unwrap();
    manifest["dependencies"][0]["manifest"] = serde_json::json!(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap()
    );
    let source = root.join("hirsel.json");
    std::fs::write(&source, manifest.to_string()).unwrap();
    let grants = serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[{"path":"data","host":data,"access":"read"}]}}).to_string();
    let tape = root.join("recording.tape");
    let recorded = command()
        .arg("run")
        .arg(&source)
        .arg("--grants")
        .arg(&grants)
        .arg("--record")
        .arg(&tape)
        .args(["--", "input.csv", "1", "--grain", "3"])
        .output()
        .unwrap();
    success(&recorded);
    assert_eq!(std::fs::read(data.join("input.csv")).unwrap(), input);
    let original = std::fs::read(&tape).unwrap();
    std::fs::rename(&data, root.join("unavailable")).unwrap();
    let replayed = command()
        .arg("run")
        .arg(&source)
        .arg("--replay")
        .arg(&tape)
        .output()
        .unwrap();
    success(&replayed);
    assert!(!data.exists());
    assert_eq!(std::fs::read(&tape).unwrap(), original);
    std::fs::rename(root.join("unavailable"), &data).unwrap();
    for (args, error) in [
        (vec!["input.csv", "1", "--grain", "0"], "Grain"),
        (vec!["input.csv", "2", "--grain", "3"], "Columns"),
        (vec!["input.csv", "one", "--grain", "3"], "Usage"),
        (vec!["input.csv", "1", "--unknown", "3"], "Usage"),
    ] {
        let output = command()
            .arg("run")
            .arg(&source)
            .arg("--grants")
            .arg(&grants)
            .arg("--")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(error),
            "{output:?}"
        );
    }
    std::fs::write(data.join("input.csv"), vec![b'x'; 262145]).unwrap();
    let output = command()
        .arg("run")
        .arg(&source)
        .arg("--grants")
        .arg(&grants)
        .args(["--", "input.csv", "1", "--grain", "3"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Limit"),
        "{output:?}"
    );
}
