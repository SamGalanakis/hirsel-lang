//! Complete rpcbatch orchestration with independent literal provider vectors.
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn package(root: &Path) -> PathBuf {
    fs::write(
        root.join("main.hir"),
        include_str!("../../../../examples/rpcbatch/main.hir"),
    )
    .unwrap();
    fs::write(
        root.join("model.hir"),
        include_str!("../../../../examples/rpcbatch/model.hir"),
    )
    .unwrap();
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/rpcbatch/hirsel.json")).unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    manifest["dependencies"][0]["manifest"] = repo
        .join("stdlib/std@1/hirsel.json")
        .to_string_lossy()
        .as_ref()
        .into();
    let protocol = root.join("protocol");
    fs::create_dir(&protocol).unwrap();
    fs::write(
        protocol.join("main.hir"),
        include_str!("../../../../examples/localrpc/main.hir"),
    )
    .unwrap();
    fs::write(
        protocol.join("model.hir"),
        include_str!("../../../../examples/localrpc/model.hir"),
    )
    .unwrap();
    let mut dependency: Value =
        serde_json::from_str(include_str!("../../../../examples/localrpc/hirsel.json")).unwrap();
    dependency["dependencies"][0]["manifest"] = manifest["dependencies"][0]["manifest"].clone();
    let dependency_path = protocol.join("hirsel.json");
    fs::write(&dependency_path, dependency.to_string()).unwrap();
    manifest["dependencies"][1]["manifest"] = dependency_path.to_string_lossy().as_ref().into();
    let selection = root.join("hirsel.json");
    fs::write(&selection, manifest.to_string()).unwrap();
    selection
}

fn evidence(label: &str) -> PathBuf {
    let parent = std::env::var_os("RPCBATCH_27_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(parent)
        .unwrap()
        .keep()
}

fn semantic(part: usize) {
    hirsel_engine::with_compiler_stack(|| {
        let root = evidence(&format!("semantic-{part}-"));
        let selection = package(&root);
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let vectors: Vec<Value> = serde_json::from_str(include_str!(
            "../../../../examples/rpcbatch/fixtures/workflows.json"
        ))
        .unwrap();
        let mut completed = 0;
        for (index, vector) in vectors.iter().enumerate().filter(|(i, _)| i % 4 == part) {
            if vector["disposition"] != "RUN" {
                eprintln!(
                    "RPCBATCH_27_INCOMPLETE {} interpreter/checked-h: {}",
                    vector["id"], vector["reason"]
                );
                continue;
            }
            let g = &vector["grants"];
            let mut world = hirsel_engine::SimulationConfig::default();
            world.args = serde_json::from_value(g["args"].clone()).unwrap();
            world.net = Some(serde_json::from_value(g["io"]["net"].clone()).unwrap());
            world.projections.insert("monotonic_clock".into());
            world.grants.insert("MonotonicClock".into());
            let mut previous = None;
            for engine in ["interpreter", "checked-h"] {
                eprintln!("RPCBATCH_27_STARTED {} {engine}", vector["id"]);
                let execution = if engine == "interpreter" {
                    hirsel_engine::run_observed_snapshot(&mut loaded.engine, &config, world.clone())
                } else {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world.clone())
                }
                .unwrap();
                fs::write(
                    root.join(format!("{index}-{engine}.txt")),
                    format!("{execution:#?}"),
                )
                .unwrap();
                let observable = execution.observation.as_ref().unwrap();
                let observed = hirsel_driver::native::Observation::of_reference(observable);
                assert_eq!(
                    observed.exit_code,
                    vector["code"].as_i64().map(|n| n as i32),
                    "{} {engine}: {observed:?}; evidence {}",
                    vector["id"],
                    root.display()
                );
                assert_eq!(
                    observed.output,
                    vector["stdout"].as_str().unwrap().as_bytes(),
                    "{} {engine}; evidence {}",
                    vector["id"],
                    root.display()
                );
                assert!(
                    observed.failures.is_empty(),
                    "{} {engine}: {observed:?}",
                    vector["id"]
                );
                if let Some(reference) = &previous {
                    assert_eq!(
                        observable, reference,
                        "{} full provider/effect history",
                        vector["id"]
                    );
                }
                previous = Some(observable.clone());
                completed += 1;
                eprintln!("RPCBATCH_27_PASS {} {engine}", vector["id"]);
            }
        }
        assert!(completed > 0);
    });
}

#[test]
fn rpcbatch_27_semantic_0() {
    semantic(0);
}
#[test]
fn rpcbatch_27_semantic_1() {
    semantic(1);
}
#[test]
fn rpcbatch_27_semantic_2() {
    semantic(2);
}
#[test]
fn rpcbatch_27_semantic_3() {
    semantic(3);
}

#[test]
fn rpcbatch_27_native_and_live_peers() {
    let root = evidence("native-");
    let selection = package(&root);
    fs::write(
        root.join("workflows.json"),
        include_str!("../../../../examples/rpcbatch/fixtures/workflows.json"),
    )
    .unwrap();
    fs::write(
        root.join("verify.py"),
        include_str!("../support/rpcbatch_27/verify.py"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(root.join("verify.py"))
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--selection")
        .arg(selection)
        .arg("--output")
        .arg(root.join("receipts"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "evidence {}\n{}\n{}",
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["passes"].as_u64().unwrap()
            >= if report["only_reverse"] == true {
                9
            } else {
                40
            },
        "{report}"
    );
    eprintln!(
        "RPCBATCH_27_NATIVE_REPORT {report}; evidence {}",
        root.display()
    );
}
