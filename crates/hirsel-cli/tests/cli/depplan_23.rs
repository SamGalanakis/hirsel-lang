//! Named dependency planning, independently checked on H, checked H and V backends.
use std::{fs, path::PathBuf, process::Command};

const MAIN: &str = include_str!("../../../../examples/depplan/main.hir");
const MODEL: &str = include_str!("../../../../examples/depplan/model.hir");
const MANIFEST: &str = include_str!("../../../../examples/depplan/hirsel.json");

fn package(root: &std::path::Path, main: &str) -> PathBuf {
    fs::create_dir_all(root.join("fixtures")).unwrap();
    fs::write(root.join("main.hir"), main).unwrap();
    fs::write(root.join("model.hir"), MODEL).unwrap();
    fs::write(
        root.join("grants.json"),
        include_str!("../../../../examples/depplan/grants.json"),
    )
    .unwrap();
    let mut manifest: serde_json::Value = serde_json::from_str(MANIFEST).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    let selection = root.join("hirsel.json");
    fs::write(&selection, manifest.to_string()).unwrap();
    selection
}

#[test]
fn depplan_23_named_manifest_three_engines_and_effects() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    package(root, MAIN);
    for (name, source) in [
        (
            "release.json",
            include_str!("../../../../examples/depplan/fixtures/release.json"),
        ),
        (
            "invalid.json",
            include_str!("../../../../examples/depplan/fixtures/invalid.json"),
        ),
        (
            "vectors.json",
            include_str!("../../../../examples/depplan/fixtures/vectors.json"),
        ),
    ] {
        fs::write(root.join("fixtures").join(name), source).unwrap();
    }
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let script = root.join("verify.py");
    fs::write(&script, include_str!("../support/depplan_23/verify.py")).unwrap();
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
    eprintln!("depplan_23 acceptance: {result}");
    assert!(result["cases"].as_u64().unwrap() >= 35, "{result}");
    assert_eq!(result["engines"], 3);
    assert_eq!(
        result["executed"],
        result["cases"].as_u64().unwrap() * 3 + 3
    );
    assert_eq!(result["cleanup"], true);
    let private_path = root.to_path_buf();
    scratch.close().unwrap();
    assert!(!private_path.exists());
}

#[test]
fn depplan_23_pure_model_checked_heap() {
    hirsel_engine::with_compiler_stack(|| {
        use hirsel_driver::native::*;
        let scratch = tempfile::tempdir().unwrap();
        // A pure entry reaches both decode and plan, including error payloads.
        let source = r##"
pub fn main() -> std@1/process.ExitCode {
  let Ok(manifest) = model.decode(unique br#"{"nodes":[{"name":"deploy","prerequisites":["build","build"]},{"name":"build","prerequisites":[]}]}"#) else { return std@1/process.exit_code(2u8) }
  let Ok(plan) = model.plan(manifest) else { return std@1/process.exit_code(3u8) }
  if plan.order != ["build", "deploy"] { return std@1/process.exit_code(4u8) }
  let Ok(cyclic) = model.decode(unique br#"{"nodes":[{"name":"a","prerequisites":["b"]},{"name":"b","prerequisites":["a"]},{"name":"downstream","prerequisites":["b"]}]}"#) else { return std@1/process.exit_code(5u8) }
  let Error(blocked) = model.plan(cyclic) else { return std@1/process.exit_code(6u8) }
  let report = model.report(blocked)
  if report.error != "blocked" || report.blocked != ["a", "b", "downstream"] { return std@1/process.exit_code(7u8) }
  let Ok(unknown) = model.decode(unique br#"{"nodes":[{"name":"deploy","prerequisites":["absent"]}]}"#) else { return std@1/process.exit_code(8u8) }
  let Error(error) = model.plan(unknown) else { return std@1/process.exit_code(9u8) }
  let unknown_report = model.report(error)
  if unknown_report.error != "unknown_name" || unknown_report.node != "deploy" || unknown_report.reference != "absent" { return std@1/process.exit_code(10u8) }
  std@1/process.SUCCESS
}
"##;
        let manifest = package(scratch.path(), source);
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap();
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
}
