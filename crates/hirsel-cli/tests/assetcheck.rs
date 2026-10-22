//! A standalone integration target for the offline release-asset verifier.
hirsel_guard::install!();

use std::path::Path;
use std::process::Command;

#[test]
fn assetcheck_host_and_cross_engine_replay() {
    let scratch = std::env::temp_dir().join(format!(
        "hirsel-assetcheck-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&scratch).unwrap();
    let package = scratch.join("package");
    std::fs::create_dir(&package).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/assetcheck/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    for (name, source) in [
        (
            "main.hir",
            include_str!("../../../examples/assetcheck/main.hir"),
        ),
        (
            "grants.json",
            include_str!("../../../examples/assetcheck/grants.json"),
        ),
    ] {
        std::fs::write(package.join(name), source).unwrap();
    }
    std::fs::write(package.join("hirsel.json"), manifest.to_string()).unwrap();
    let script = scratch.join("verify.py");
    std::fs::write(
        &script,
        include_str!("../../../examples/verify-dogfood7.py"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(script)
        .args(["--hirsel", env!("CARGO_BIN_EXE_hirsel"), "--root"])
        .arg(package)
        .arg("--output")
        .arg(scratch.join("evidence"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "evidence retained at {}\n{}\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("4 package tests x 3 engines"), "{stdout}");
    assert!(
        stdout.contains("exact command observations passed"),
        "{stdout}"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
