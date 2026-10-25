//! Sequential scenario assertions and full offline replay on H, LLVM, Cranelift.
use std::{path::Path, process::Command};

#[test]
fn httpcheck_15_scenarios_and_offline_replay() {
    run(None);
}

#[test]
fn httpcheck_15_h_body_cap() {
    run(Some("h-body-cap"));
}

fn run(case: Option<&str>) {
    // Retain failures, including setup/timeout cells, outside the source tree.
    let scratch = tempfile::Builder::new()
        .prefix("hirsel-httpcheck-15-")
        .tempdir()
        .unwrap()
        .keep();
    let package = scratch.join("package");
    std::fs::create_dir(&package).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/httpcheck/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    std::fs::write(package.join("hirsel.json"), manifest.to_string()).unwrap();
    std::fs::write(
        package.join("main.hir"),
        include_bytes!("../../../../examples/httpcheck/main.hir"),
    )
    .unwrap();
    let helper = scratch.join("verify.py");
    std::fs::write(&helper, include_bytes!("../support/httpcheck_15/verify.py")).unwrap();
    let mut command = Command::new("python3");
    command
        .arg(&helper)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(&package)
        .arg("--output")
        .arg(scratch.join("evidence"));
    if let Some(case) = case {
        command.arg("--case").arg(case);
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "evidence: {}\n{stdout}\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    if case.is_some() {
        assert!(
            stdout.contains("case=h-body-cap engines={'h': 1} observations=1"),
            "{stdout}"
        );
        println!("{stdout}evidence: {}", scratch.display());
        return;
    }
    assert!(
        stdout.contains("engines={'h': 1, 'llvm': 1, 'cranelift': 1}"),
        "{stdout}"
    );
    assert!(stdout.contains("records=9 replays=27"), "{stdout}");
    println!("{stdout}evidence: {}", scratch.display());
}
