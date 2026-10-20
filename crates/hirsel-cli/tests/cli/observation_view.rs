use std::process::Command;

#[test]
fn offline_view_requires_capture_identity_and_writes_logical_perfetto() {
    let root = std::env::temp_dir().join(format!("hirsel-view-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let tape = root.join("ordinary.tape");
    let out = root.join("view.json");
    std::fs::write(
        &tape,
        include_bytes!("../../../hirsel-driver/tests/fixtures/observation-view/ordinary.tape"),
    )
    .unwrap();
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
