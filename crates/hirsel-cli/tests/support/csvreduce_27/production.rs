//! Ordinary production admission, immutable tapes and full semantic event order.
//! Placement observations require the separate live control handshake campaign.
use std::path::Path;
use std::process::{Command, Output};

fn clean(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("HIRSEL_RT_") {
            command.env_remove(key);
        }
    }
    command.env("HIRSEL_JOBS", "4");
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
fn semantic_tape(path: &Path) -> hirsel_rt::launch::Recording {
    let mut tape = hirsel_rt::host_launch::read_recording(path).unwrap();
    let binding = tape.binding.as_mut().unwrap();
    binding.admitted_at = 0;
    binding.placement = None;
    tape
}
pub fn ordered_replay() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let data = root.join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::write(
        data.join("input.csv"),
        include_bytes!("../../../../../examples/csvreduce/fixtures/mixed.csv"),
    )
    .unwrap();
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
    let cli = Path::new(env!("CARGO_BIN_EXE_hirsel"));
    let mut recordings = Vec::new();
    let mut baseline = None;
    for backend in ["llvm", "cranelift"] {
        let exe = root.join(backend);
        let built = clean(cli)
            .env("HIRSEL_JOBS", "1")
            .args(["build", "--backend", backend])
            .arg(&source)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(built.status.success(), "{built:?}");
        for width in [1, 2, 4] {
            let cell = format!("{backend}-{width}");
            let tape = root.join(format!("{cell}.tape"));
            let trace = root.join(format!("{cell}.jsonl"));
            let grants = serde_json::json!({"io":{"tasks":"tasks.cooperative@1","stdout":"stdout.bytes@1","dirs":[{"path":"data","host":data,"access":"read"}]},"placement":{"cores":width,"commit":"shared","reorder":"auto"}}).to_string();
            let output = clean(&exe)
                .arg("--hirsel-trace-events")
                .arg(&trace)
                .arg("--hirsel-record")
                .arg(&tape)
                .arg("--hirsel-grants")
                .arg(grants)
                .args(["input.csv", "1", "--grain", "3"])
                .output()
                .unwrap();
            success(&output);
            let observations = (semantic_tape(&tape), std::fs::read(&trace).unwrap());
            assert!(!observations.1.is_empty());
            if let Some(baseline) = &baseline {
                assert_eq!(&observations, baseline, "{cell}");
            } else {
                baseline = Some(observations);
            }
            recordings.push((cell, tape, std::fs::read(&trace).unwrap()));
        }
    }
    std::fs::rename(&data, root.join("unavailable")).unwrap();
    for (cell, tape, recorded_trace) in recordings {
        let original = std::fs::read(&tape).unwrap();
        let output = clean(cli)
            .arg("run")
            .arg(&source)
            .arg("--replay")
            .arg(&tape)
            .output()
            .unwrap();
        success(&output);
        for backend in ["llvm", "cranelift"] {
            for width in [1, 2, 4] {
                let trace = root.join(format!("{cell}-replay-{backend}-{width}.jsonl"));
                // Replay-only topology hook; production recordings above strip it.
                let output = clean(&root.join(backend))
                    .env("HIRSEL_RT_WORKERS", width.to_string())
                    .arg("--hirsel-trace-events")
                    .arg(&trace)
                    .args([
                        "--hirsel-replay-policy",
                        "{\"commit\":\"shared\",\"reorder\":\"auto\"}",
                    ])
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .output()
                    .unwrap();
                success(&output);
                assert_eq!(std::fs::read(trace).unwrap(), recorded_trace);
            }
        }
        assert_eq!(std::fs::read(tape).unwrap(), original);
        assert!(!data.exists());
        eprintln!(
            "csvreduce production {cell}: recorded + interpreter replay + six native replay cells PASS"
        );
    }
}
