//! Independent TAR/ZIP oracle, three engines and grant-free offline replay.
use std::{fs, path::PathBuf, process::Command};

#[test]
fn archiveaudit_15_independent_archives_engines_and_offline_replay() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        include_str!("../../../../examples/archiveaudit/main.hir"),
    )
    .unwrap();
    let mut package: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../examples/archiveaudit/hirsel.json"
    ))
    .unwrap();
    package["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    fs::write(root.join("hirsel.json"), package.to_string()).unwrap();
    let script = root.join("verify.py");
    fs::write(
        &script,
        include_str!("../support/archiveaudit_15/verify.py"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(script)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(root)
        .arg("--output")
        .arg(root.join("evidence"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result["cases"].as_u64().unwrap() >= 60, "{result}");
    assert!(result["recordings"].as_u64().unwrap() >= 30, "{result}");
    assert_eq!(
        result["replays"],
        result["recordings"].as_u64().unwrap() * 3
    );
}
