//! One build per backend, bounded loopback HTTP, all-engine offline replay.
hirsel_guard::install!();

use std::path::Path;
use std::process::Command;

#[test]
fn httpaggregate_9_end_to_end() {
    let scratch = tempfile::Builder::new()
        .prefix("hirsel-httpaggregate-9-")
        .tempdir()
        .unwrap()
        .keep();
    let package = scratch.join("package");
    std::fs::create_dir_all(package.join("fixtures")).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/httpaggregate/hirsel.json")).unwrap();
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
            std::fs::write(package.join($name), include_bytes!(concat!(
                "../../../examples/httpaggregate/", $name))).unwrap();
        )*};
    }
    copy_app!(
        "main.hir",
        "model.hir",
        "fixtures/first.json",
        "fixtures/second.json",
        "fixtures/empty.json",
        "fixtures/duplicate.json",
        "fixtures/missing.json",
        "fixtures/unknown.json",
        "fixtures/type.json",
        "fixtures/bad-json.json",
        "fixtures/combined.out",
        "fixtures/first.out",
        "fixtures/empty.out",
        "fixtures/help.out"
    );
    let helper = scratch.join("verify.py");
    std::fs::write(&helper, include_bytes!("support/httpaggregate_9/verify.py")).unwrap();
    let output = Command::new("python3")
        .arg(&helper)
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
    assert!(
        stdout.contains("135 exact observations; 6 records; 18 replays"),
        "{stdout}"
    );
    assert!(
        stdout.contains("builds={'llvm': 1, 'cranelift': 1}"),
        "{stdout}"
    );
    println!("{stdout}evidence: {}", scratch.display());
}
