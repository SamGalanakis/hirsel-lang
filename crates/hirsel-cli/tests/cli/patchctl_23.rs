//! Read-only patches: independent oracle, actual CLI and two native backends.
use std::{fs, path::PathBuf, process::Command};

fn package(root: &std::path::Path) -> PathBuf {
    fs::create_dir_all(root.join("fixtures")).unwrap();
    fs::write(
        root.join("main.hir"),
        include_str!("../../../../examples/patchctl/main.hir"),
    )
    .unwrap();
    fs::write(
        root.join("model.hir"),
        include_str!("../../../../examples/patchctl/model.hir"),
    )
    .unwrap();
    for (name, source) in [
        (
            "manifest.hir",
            include_str!("../../../../examples/patchctl/manifest.hir"),
        ),
        (
            "plan.hir",
            include_str!("../../../../examples/patchctl/plan.hir"),
        ),
        (
            "apply.hir",
            include_str!("../../../../examples/patchctl/apply.hir"),
        ),
        (
            "publish.hir",
            include_str!("../../../../examples/patchctl/publish.hir"),
        ),
        (
            "report.hir",
            include_str!("../../../../examples/patchctl/report.hir"),
        ),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    for (name, source) in [
        (
            "old.txt",
            include_str!("../../../../examples/patchctl/fixtures/old.txt"),
        ),
        (
            "new.txt",
            include_str!("../../../../examples/patchctl/fixtures/new.txt"),
        ),
        (
            "review.patch",
            include_str!("../../../../examples/patchctl/fixtures/review.patch"),
        ),
        (
            "stale.patch",
            include_str!("../../../../examples/patchctl/fixtures/stale.patch"),
        ),
    ] {
        fs::write(root.join("fixtures").join(name), source).unwrap();
    }
    fs::write(
        root.join("grants.json"),
        include_str!("../../../../examples/patchctl/grants.json"),
    )
    .unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/patchctl/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    let path = root.join("hirsel.json");
    fs::write(&path, manifest.to_string()).unwrap();
    path
}

#[test]
fn patchctl_23_oracle_three_engines_and_cleanup() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    package(root);
    let script = root.join("verify.py");
    fs::write(&script, include_str!("../support/patchctl_23/verify.py")).unwrap();
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
    assert!(result["observations"].as_u64().unwrap() >= 180, "{result}");
    assert_eq!(result["vectors"], 15);
    assert_eq!(result["cleanup"], true);
    scratch.close().unwrap();
}

#[test]
fn patchctl_23_pure_model_checked_heap() {
    let scratch = tempfile::tempdir().unwrap();
    let package_manifest = package(scratch.path());
    fs::write(
        scratch.path().join("main.hir"),
        r#"pub fn main() -> Result[(), model.InvalidInput] {
  let single = model.execute(Diff(), "a\nold", "a\nnew")?
  let applied = model.execute(Apply(), "a\nold", single.text)?
  failure.check(applied.text == "a\nnew", Bounds())
  let rejected = model.execute(Apply(), "stale\n", "--- old\n+++ new\n@@ -1 +1 @@\n-a\n+b\n")
  match rejected {
    Error(Rejected(indexes)) => failure.check(indexes == [0u64], Bounds()),
    _ => failure.check(false, Bounds()),
  }
  Ok(())
}"#,
    )
    .unwrap();
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native::*;
        let mut loaded = hirsel_driver::load_program(&package_manifest).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let heap = run_heap(&ready, &loaded.entry, &Sites::new(validated.snapshot())).unwrap();
        assert!(
            heap.observation.failures.is_empty(),
            "{:?}",
            heap.observation
        );
        assert!(heap.observation.output.is_empty());
        assert_eq!(heap.counters.live_heap_bytes, 0);
    });
    scratch.close().unwrap();
}
