//! Bounded RFC6902 CLI publication, independent vectors, three engines and cleanup.
use std::{fs, path::PathBuf, process::Command};

fn package(root: &std::path::Path) -> PathBuf {
    fs::write(
        root.join("main.hir"),
        include_str!("../../../../examples/jsonpatchctl/main.hir"),
    )
    .unwrap();
    fs::write(
        root.join("model.hir"),
        include_str!("../../../../examples/jsonpatchctl/model.hir"),
    )
    .unwrap();
    let mut package: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../examples/jsonpatchctl/hirsel.json"
    ))
    .unwrap();
    package["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, package.to_string()).unwrap();
    manifest
}

#[test]
fn jsonpatchctl_23_vectors_engines_and_cleanup() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().to_path_buf();
    package(&root);
    fs::create_dir(root.join("fixtures")).unwrap();
    for (name, contents) in [
        (
            "deployment.json",
            include_str!("../../../../examples/jsonpatchctl/fixtures/deployment.json"),
        ),
        (
            "promote.patch.json",
            include_str!("../../../../examples/jsonpatchctl/fixtures/promote.patch.json"),
        ),
        (
            "stale.patch.json",
            include_str!("../../../../examples/jsonpatchctl/fixtures/stale.patch.json"),
        ),
        (
            "vectors.json",
            include_str!("../../../../examples/jsonpatchctl/fixtures/vectors.json"),
        ),
    ] {
        fs::write(root.join("fixtures").join(name), contents).unwrap();
    }
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let script = root.join("verify.py");
    fs::write(
        &script,
        include_str!("../support/jsonpatchctl_23/verify.py"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(script)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(&root)
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
    assert_eq!(result["cases"], 42);
    assert_eq!(result["observations"], 126);
    assert_eq!(result["pure_tests"], 1);
    assert_eq!(result["cleanup"], true);
    scratch.close().unwrap();
    assert!(!root.exists(), "private test files were not removed");
}

#[test]
fn jsonpatchctl_23_pure_model_checked_heap() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().to_path_buf();
    let selection = package(&root);
    fs::write(
        root.join("main.hir"),
        "pub fn main() -> Result[(), String] uses {} { model.verify() }\n",
    )
    .unwrap();
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native::*;
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let heap = run_heap(&ready, &loaded.entry, &Sites::new(validated.snapshot())).unwrap();
        assert_eq!(
            heap.observation.exit_code,
            Some(0),
            "{:?}",
            heap.observation
        );
        assert!(heap.observation.output.is_empty() && heap.observation.failures.is_empty());
        assert_eq!(heap.counters.live_heap_bytes, 0);
    });
    scratch.close().unwrap();
    assert!(!root.exists());
}
