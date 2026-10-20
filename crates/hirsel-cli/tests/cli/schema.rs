//! Adapted from cli/replay.rs: record real file I/O, remove the mount, replay.
use std::path::Path;
use std::process::{Command, Stdio};

#[test]
fn schema_native_recording_replays_identical_issues_and_checked_bytes_without_host_input() {
    let marker = super::file("");
    let fixture = marker.with_extension("schema-replay");
    std::fs::remove_file(marker).unwrap();
    std::fs::create_dir_all(fixture.join("data")).unwrap();
    std::fs::write(fixture.join("data/input.json"), r#"[1,256,"bad"]"#).unwrap();
    std::fs::write(
        fixture.join("main.hir"),
        include_str!("../fixtures/schema-replay/main.hir"),
    )
    .unwrap();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    std::fs::write(
        fixture.join("hirsel.json"),
        serde_json::json!({"schema":1,"package":{"name":"schema_replay","version":"1"},
            "sources":[{"path":"main.hir","module":[]}],
            "dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],
            "entry":{"module":[],"name":"main"}})
        .to_string(),
    )
    .unwrap();
    let source = fixture.join("hirsel.json");
    let grants = fixture.join("grants.json");
    std::fs::write(
        &grants,
        serde_json::json!({"io":{"stdout":"stdout.bytes@1",
            "dirs":[{"path":"data","host":fixture.join("data"),"access":"read"}]},"args":[]})
        .to_string(),
    )
    .unwrap();
    let tape = fixture.join("native.tape");
    let recorded = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .args(["--native", "--grants"])
        .arg(&grants)
        .arg("--record")
        .arg(&tape)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(recorded.status.success(), "{recorded:?}");
    assert!(recorded.stderr.is_empty(), "{recorded:?}");
    assert_eq!(recorded.stdout, b"[1]: Range (u8)\n[2]: Type (u8)\n7\n");
    // No file, directory, stdin or grants remain available at replay time.
    std::fs::remove_dir_all(fixture.join("data")).unwrap();
    std::fs::remove_file(grants).unwrap();
    let replayed = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--replay")
        .arg(&tape)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(replayed.status.success(), "{replayed:?}");
    assert!(replayed.stderr.is_empty(), "{replayed:?}");
    assert_eq!(replayed.stdout, recorded.stdout);
    std::fs::remove_dir_all(fixture).unwrap();
}

// This campaign exceeds the corpus's 1M-step allowance. Keep the generated
// cases in a Rust test with an explicit unbounded step budget instead.
#[test]
fn schema_seeded_mapping_laws_in_interpreter_and_native_backends() {
    let marker = super::file("");
    let fixture = marker.with_extension("schema-laws");
    std::fs::remove_file(marker).unwrap();
    std::fs::create_dir_all(&fixture).unwrap();
    std::fs::write(
        fixture.join("main.hir"),
        include_str!("../fixtures/schema-laws/main.hir"),
    )
    .unwrap();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let manifest = fixture.join("hirsel.json");
    std::fs::write(
        &manifest,
        serde_json::json!({"schema":1,"package":{"name":"schema_laws","version":"1"},
            "sources":[{"path":"main.hir","module":[]}],
            "dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},
                "expose":[{"head":"schema","module":["schema"]}]}],
            "entry":{"module":[],"name":"trial"}})
        .to_string(),
    )
    .unwrap();
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
        command.args(["test", "--seed", "1"]);
        if let Some(backend) = backend {
            command.args(["--native", "--backend", backend]);
        } else {
            command.args(["--max-steps", "unbounded"]);
        }
        let output = command.arg(&manifest).output().unwrap();
        assert!(output.status.success(), "{backend:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{backend:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed; 1 total"),
            "{backend:?}: {output:?}"
        );
    }
    std::fs::remove_dir_all(fixture).unwrap();
}
