//! Stateful stdin responses, bounded candidates and offline cross-engine replay.
hirsel_guard::install!();
use std::path::Path;
use std::process::Command;

#[test]
fn docsession_11_end_to_end() {
    let parent = std::env::var_os("HIRSEL_DOCSESSION_EVIDENCE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&parent).unwrap();
    let scratch = tempfile::Builder::new()
        .prefix("docsession-11-")
        .tempdir_in(parent)
        .unwrap()
        .keep();
    let package = scratch.join("package");
    std::fs::create_dir_all(package.join("fixtures")).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/docsession/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    std::fs::write(package.join("hirsel.json"), manifest.to_string()).unwrap();
    macro_rules! copy_app {
        ($($name:literal),* $(,)?) => {$(
            std::fs::write(package.join($name), include_bytes!(concat!("../../../examples/docsession/", $name))).unwrap();
        )*};
    }
    copy_app!(
        "main.hir",
        "model.hir",
        "fixtures/transactions.in",
        "fixtures/transactions.out",
        "fixtures/errors.in",
        "fixtures/errors.out"
    );
    let helper = scratch.join("verify.py");
    std::fs::write(&helper, include_bytes!("support/docsession_11/verify.py")).unwrap();
    let mut command = Command::new("python3");
    if let Some(reuse) = std::env::var_os("HIRSEL_DOCSESSION_REUSE") {
        command.arg(&helper).arg("--reuse").arg(reuse);
    } else {
        command.arg(&helper);
    }
    let output = command
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(&package)
        .arg("--output")
        .arg(scratch.join("evidence"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "evidence: {}\n{}\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("6 records; 18 replays"), "{stdout}");
    assert!(
        stdout.contains("builds={'llvm': 1, 'cranelift': 1}"),
        "{stdout}"
    );
    println!("{stdout}evidence: {}", scratch.display());
}
