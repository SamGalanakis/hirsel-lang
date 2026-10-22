//! Focused app-only integration; each native app executable is built once.
hirsel_guard::install!();
use std::path::Path;
use std::process::Command;

fn verify_group(group: &str, observations: u64) {
    let scratch = std::env::temp_dir().join(format!(
        "hirsel-configctl-{}-{}-{}",
        group,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let package = scratch.join("package");
    std::fs::create_dir_all(package.join("fixtures")).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../examples/configctl/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    std::fs::write(package.join("hirsel.json"), manifest.to_string()).unwrap();
    for (name, data) in [
        (
            "main.hir",
            include_str!("../../../examples/configctl/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../examples/configctl/model.hir"),
        ),
        (
            "fixtures/-dogfood8-name.json",
            include_str!("../../../examples/configctl/fixtures/-dogfood8-name.json"),
        ),
        (
            "fixtures/dogfood8-denied.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-denied.json"),
        ),
        (
            "fixtures/dogfood8-diff-help.out",
            include_str!("../../../examples/configctl/fixtures/dogfood8-diff-help.out"),
        ),
        (
            "fixtures/dogfood8-diff.out",
            include_str!("../../../examples/configctl/fixtures/dogfood8-diff.out"),
        ),
        (
            "fixtures/dogfood8-duplicate.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-duplicate.json"),
        ),
        (
            "fixtures/dogfood8-empty.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-empty.json"),
        ),
        (
            "fixtures/dogfood8-equal.out",
            include_str!("../../../examples/configctl/fixtures/dogfood8-equal.out"),
        ),
        (
            "fixtures/dogfood8-grants.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-grants.json"),
        ),
        (
            "fixtures/dogfood8-help.out",
            include_str!("../../../examples/configctl/fixtures/dogfood8-help.out"),
        ),
        (
            "fixtures/dogfood8-invalid11.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-invalid11.json"),
        ),
        (
            "fixtures/dogfood8-malformed.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-malformed.json"),
        ),
        (
            "fixtures/dogfood8-migrate-help.out",
            include_str!("../../../examples/configctl/fixtures/dogfood8-migrate-help.out"),
        ),
        (
            "fixtures/dogfood8-migrated-ten.out",
            include_str!("../../../examples/configctl/fixtures/dogfood8-migrated-ten.out"),
        ),
        (
            "fixtures/dogfood8-migrated-zero.out",
            include_str!("../../../examples/configctl/fixtures/dogfood8-migrated-zero.out"),
        ),
        (
            "fixtures/dogfood8-missing.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-missing.json"),
        ),
        (
            "fixtures/dogfood8-type.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-type.json"),
        ),
        (
            "fixtures/dogfood8-unknown.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-unknown.json"),
        ),
        (
            "fixtures/dogfood8-v1-ten.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-v1-ten.json"),
        ),
        (
            "fixtures/dogfood8-v1-zero.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-v1-zero.json"),
        ),
        (
            "fixtures/dogfood8-v2-equal.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-v2-equal.json"),
        ),
        (
            "fixtures/dogfood8-v2.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-v2.json"),
        ),
        (
            "fixtures/dogfood8-validate-help.out",
            include_str!("../../../examples/configctl/fixtures/dogfood8-validate-help.out"),
        ),
        (
            "fixtures/dogfood8-verify.py",
            include_str!("../../../examples/configctl/fixtures/dogfood8-verify.py"),
        ),
        (
            "fixtures/dogfood8-version.json",
            include_str!("../../../examples/configctl/fixtures/dogfood8-version.json"),
        ),
    ] {
        std::fs::write(package.join(name), data).unwrap();
    }
    let output = Command::new("python3")
        .arg(package.join("fixtures/dogfood8-verify.py"))
        .args(["--hirsel", env!("CARGO_BIN_EXE_hirsel"), "--root"])
        .arg(&package)
        .arg("--output")
        .arg(scratch.join("evidence"))
        .arg("--group")
        .arg(group)
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
        stdout.contains(&format!("{observations} exact observations")),
        "{stdout}"
    );
    assert!(
        stdout.contains("builds={'llvm': 1, 'cranelift': 1}"),
        "{stdout}"
    );
    if group == "replay" {
        assert!(stdout.contains("9 package test executions"), "{stdout}");
        assert!(stdout.contains("6 records; 18 replays"), "{stdout}");
    }
    // Preserve all exact command evidence, including on success.
    println!("{}evidence: {}", stdout, scratch.display());
}

#[test]
fn configctl_8_cli_arguments() {
    verify_group("cli", 69);
}

#[test]
fn configctl_8_data_validation() {
    verify_group("data", 66);
}

#[test]
fn configctl_8_record_replay() {
    verify_group("replay", 24);
}
