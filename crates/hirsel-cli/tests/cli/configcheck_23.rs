//! One application model, upstream vectors and bounded real CLI execution.
use std::{fs, path::PathBuf, process::Command};

fn package() -> tempfile::TempDir {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(
        root.join("main.hir"),
        include_str!("../../../../examples/configcheck/main.hir"),
    )
    .unwrap();
    fs::write(
        root.join("model.hir"),
        include_str!("../../../../examples/configcheck/model.hir"),
    )
    .unwrap();
    fs::create_dir(root.join("fixtures")).unwrap();
    for (name, source) in [
        (
            "service.toml",
            include_str!("../../../../examples/configcheck/fixtures/service.toml"),
        ),
        (
            "service.yaml",
            include_str!("../../../../examples/configcheck/fixtures/service.yaml"),
        ),
        (
            "invalid.yaml",
            include_str!("../../../../examples/configcheck/fixtures/invalid.yaml"),
        ),
        (
            "quoted-dots.toml",
            include_str!("../../../../examples/configcheck/fixtures/quoted-dots.toml"),
        ),
        (
            "nested.yaml",
            include_str!("../../../../examples/configcheck/fixtures/nested.yaml"),
        ),
    ] {
        fs::write(root.join("fixtures").join(name), source).unwrap();
    }
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/configcheck/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    fs::write(root.join("hirsel.json"), manifest.to_string()).unwrap();
    scratch
}

#[test]
fn configcheck_23_cli_three_engines_and_independent_oracles() {
    let scratch = package();
    let root = scratch.path();
    let script = root.join("verify.py");
    fs::write(&script, include_str!("../support/configcheck_23/verify.py")).unwrap();
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
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
    println!(
        "configcheck_23 matrix: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result["cases"].as_u64().unwrap() >= 60, "{result}");
    assert_eq!(
        result["observations"],
        result["cases"].as_u64().unwrap() * 3
    );
    assert_eq!(
        result["engines"],
        serde_json::json!(["h", "llvm", "cranelift"])
    );
    assert_eq!(result["cleanup"], true);
    assert_eq!(result["vector_observations"], 3);
    let path = scratch.path().to_owned();
    scratch.close().unwrap();
    assert!(!path.exists(), "private package cleanup");
}

#[test]
fn configcheck_23_pure_reference_vectors_checked_h() {
    use hirsel_driver::*;
    use hirsel_engine::*;

    with_compiler_stack(|| {
        let scratch = package();
        let path = scratch.path().join("hirsel.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["entry"] = serde_json::json!({"module":["model"], "name":"reference_vectors"});
        fs::write(&path, manifest.to_string()).unwrap();
        let mut loaded = load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let plain = run_observed_snapshot(&mut loaded.engine, &config, SimulationConfig::default())
            .unwrap();
        let checked =
            run_observed_checked(&mut loaded.engine, &config, SimulationConfig::default()).unwrap();
        for outcome in [&plain.outcome, &checked.outcome] {
            assert!(
                matches!(
                    outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{outcome:?}"
            );
        }
        assert_eq!(plain.observation, checked.observation);
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let heap = native::run_heap_in(
            &ready,
            &loaded.entry,
            &native::Sites::new(validated.snapshot()),
            &native::HeapWorld {
                steps: Some(10_000_000),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            heap.observation,
            native::Observation::of_reference(plain.observation.as_ref().unwrap())
        );
        assert_eq!(heap.counters.live_heap_bytes, 0);
    });
}
