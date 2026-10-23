//! walkdir nested-tree adaptations; exact app bytes and bounded directory-read effects.
use hirsel_engine::{RunConfiguration, SimulationConfig};
use hirsel_interp::{
    library::simulation::{Fault, FileError},
    observation::Effect,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Vector {
    id: &'static str,
    input: &'static str,
    expected: &'static str,
    raw: &'static str,
    catalogue: &'static str,
}
fn vectors(caps: bool) -> Vec<Vector> {
    let all = vec![
        Vector {
            id: "cap-0",
            input: include_str!("../support/filemanifest_stream_25/cap-0.input.json"),
            expected: include_str!("../support/filemanifest_stream_25/cap-0.expected.json"),
            raw: include_str!("../support/filemanifest_stream_25/cap-0.walkdir-raw.txt"),
            catalogue: include_str!("../support/filemanifest_stream_25/cap-0.catalogue.json"),
        },
        Vector {
            id: "cap-127",
            input: include_str!("../support/filemanifest_stream_25/cap-127.input.json"),
            expected: include_str!("../support/filemanifest_stream_25/cap-127.expected.json"),
            raw: include_str!("../support/filemanifest_stream_25/cap-127.walkdir-raw.txt"),
            catalogue: include_str!("../support/filemanifest_stream_25/cap-127.catalogue.json"),
        },
        Vector {
            id: "cap-128",
            input: include_str!("../support/filemanifest_stream_25/cap-128.input.json"),
            expected: include_str!("../support/filemanifest_stream_25/cap-128.expected.json"),
            raw: include_str!("../support/filemanifest_stream_25/cap-128.walkdir-raw.txt"),
            catalogue: include_str!("../support/filemanifest_stream_25/cap-128.catalogue.json"),
        },
        Vector {
            id: "cap-129",
            input: include_str!("../support/filemanifest_stream_25/cap-129.input.json"),
            expected: include_str!("../support/filemanifest_stream_25/cap-129.expected.json"),
            raw: include_str!("../support/filemanifest_stream_25/cap-129.walkdir-raw.txt"),
            catalogue: include_str!("../support/filemanifest_stream_25/cap-129.catalogue.json"),
        },
        Vector {
            id: "late-error",
            input: include_str!("../support/filemanifest_stream_25/late-error.input.json"),
            expected: include_str!("../support/filemanifest_stream_25/late-error.expected.json"),
            raw: include_str!("../support/filemanifest_stream_25/late-error.walkdir-raw.txt"),
            catalogue: include_str!("../support/filemanifest_stream_25/late-error.catalogue.json"),
        },
        Vector {
            id: "lookahead-error",
            input: include_str!("../support/filemanifest_stream_25/lookahead-error.input.json"),
            expected: include_str!(
                "../support/filemanifest_stream_25/lookahead-error.expected.json"
            ),
            raw: include_str!("../support/filemanifest_stream_25/lookahead-error.walkdir-raw.txt"),
            catalogue: include_str!(
                "../support/filemanifest_stream_25/lookahead-error.catalogue.json"
            ),
        },
        Vector {
            id: "before-cap-error",
            input: include_str!("../support/filemanifest_stream_25/before-cap-error.input.json"),
            expected: include_str!(
                "../support/filemanifest_stream_25/before-cap-error.expected.json"
            ),
            raw: include_str!("../support/filemanifest_stream_25/before-cap-error.walkdir-raw.txt"),
            catalogue: include_str!(
                "../support/filemanifest_stream_25/before-cap-error.catalogue.json"
            ),
        },
        Vector {
            id: "selected-link",
            input: include_str!("../support/filemanifest_stream_25/selected-link.input.json"),
            expected: include_str!("../support/filemanifest_stream_25/selected-link.expected.json"),
            raw: include_str!("../support/filemanifest_stream_25/selected-link.walkdir-raw.txt"),
            catalogue: include_str!(
                "../support/filemanifest_stream_25/selected-link.catalogue.json"
            ),
        },
    ];
    all.into_iter()
        .filter(|v| v.id.starts_with("cap-") == caps)
        .collect()
}
fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
fn manifest() -> Value {
    serde_json::from_str(include_str!(
        "../support/filemanifest_stream_25/manifest.json"
    ))
    .unwrap()
}
fn check_hashes(vector: &Vector) -> Value {
    let row = manifest()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == vector.id)
        .unwrap()
        .clone();
    for (bytes, field) in [
        (vector.input, "input_sha256"),
        (vector.expected, "expected_sha256"),
        (vector.raw, "raw_sha256"),
        (vector.catalogue, "catalogue_sha256"),
    ] {
        assert_eq!(hash(bytes.as_bytes()), row[field], "{} {field}", vector.id);
    }
    row
}
fn scratch() -> PathBuf {
    let evidence = std::env::var_os("FILEMANIFEST_STREAM_25_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    tempfile::Builder::new()
        .prefix("filemanifest-stream-25-")
        .tempdir_in(evidence)
        .unwrap()
        .keep()
}
fn package(root: &Path) -> PathBuf {
    fs::write(
        root.join("main.hir"),
        include_str!("../../../../examples/filemanifest/main.hir"),
    )
    .unwrap();
    fs::write(
        root.join("model.hir"),
        include_str!("../../../../examples/filemanifest/model.hir"),
    )
    .unwrap();
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let mut package: Value = serde_json::from_str(include_str!(
        "../../../../examples/filemanifest/hirsel.json"
    ))
    .unwrap();
    package["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    let selection = root.join("hirsel.json");
    fs::write(&selection, package.to_string()).unwrap();
    selection
}
fn receipt(vector: &Vector, engine: &str, projection: &Value) {
    let row = check_hashes(vector);
    eprintln!(
        "FILEMANIFEST_STREAM_25 {}",
        json!({"id":vector.id,"engine":engine,"selected":1,"started":1,"completed":1,"passed":1,"failed":0,"unrun":0,"input_sha256":row["input_sha256"],"expected_sha256":row["expected_sha256"],"source_sha256":hash(include_bytes!("../../../../examples/filemanifest/main.hir")),"compiler":hirsel_driver::COMPILER_IDENTITY,"projection":projection})
    );
}
// Event values use the public simulator's canonical encoding: a Path ends in
// String(tag 3, little-endian byte length, UTF-8). Match only frozen candidate
// spellings; no Hirsel source/parser computes an expected value.
fn event_path(input: &[u8], candidates: &BTreeSet<String>) -> String {
    candidates
        .iter()
        .find(|path| {
            let mut tail = vec![3];
            tail.extend_from_slice(&(path.len() as u64).to_le_bytes());
            tail.extend_from_slice(path.as_bytes());
            input.ends_with(&tail)
        })
        .unwrap_or_else(|| panic!("unknown event Path: {input:?}"))
        .clone()
}
fn simulated(caps: bool) {
    let name = if caps {
        "filemanifest_stream_25::reference_checked_caps"
    } else {
        "filemanifest_stream_25::reference_checked_lazy"
    };
    // Compiler and image setup are normal build work. Only the execution child
    // is capped; it uses the already built test executable and matching image.
    if std::env::var("FILEMANIFEST_STREAM_25_CHILD").as_deref() != Ok(name) {
        let root = scratch();
        fs::write(
            root.join("capped"),
            include_str!("../../../../tools/capped"),
        )
        .unwrap();
        let result = Command::new("python3")
            .arg(root.join("capped"))
            .args(["--timeout", "60"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture", "--test-threads=1"])
            .env("FILEMANIFEST_STREAM_25_CHILD", name)
            .env("HIRSEL_JOBS", "1")
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .output()
            .unwrap();
        fs::write(root.join("stdout"), &result.stdout).unwrap();
        fs::write(root.join("stderr"), &result.stderr).unwrap();
        assert!(
            result.status.success(),
            "evidence {}: {}\n{}",
            root.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        eprintln!("{}", String::from_utf8_lossy(&result.stderr));
        fs::remove_dir_all(root).unwrap();
        return;
    }
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch();
        let selection = package(&root);
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded.check_for_execution().unwrap();
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for vector in vectors(caps) {
            check_hashes(&vector);
            let input: Value = serde_json::from_str(vector.input).unwrap();
            let expected: Value = serde_json::from_str(vector.expected).unwrap();
            let mut world = SimulationConfig {
                args: vec![
                    "scan".into(),
                    "fixtures".into(),
                    input["pattern"].as_str().unwrap().into(),
                ],
                ..Default::default()
            };
            world.mounts = [("fixtures".into(), hirsel_run_schema::Access::Read)].into();
            world.projections = ["stdout", "stderr", "dir"]
                .into_iter()
                .map(str::to_owned)
                .collect();
            for (name, bytes) in input["files"].as_object().unwrap() {
                world.files.insert(
                    format!("fixtures/{name}"),
                    bytes.as_str().unwrap().as_bytes().to_vec(),
                );
            }
            for name in input["directories"].as_array().unwrap() {
                // The host's mkdir(parents=True) creates each ancestor too.
                let mut prefix = String::from("fixtures");
                for part in name.as_str().unwrap().split('/') {
                    prefix.push('/');
                    prefix.push_str(part);
                    world.directories.insert(prefix.clone());
                }
            }
            for (name, target) in input["links"].as_object().unwrap() {
                world
                    .links
                    .insert(format!("fixtures/{name}"), target.as_str().unwrap().into());
            }
            if !input["directory_error"].is_null() {
                world.faults.insert(
                    "file.read_dir".into(),
                    [Fault::File {
                        error: FileError::Fault(1),
                        partial: 0,
                    }]
                    .into(),
                );
            }
            let mut candidates: BTreeSet<String> = [String::new()].into();
            for name in world
                .files
                .keys()
                .chain(world.directories.iter())
                .chain(world.links.keys())
            {
                if let Some(name) = name.strip_prefix("fixtures/") {
                    let mut prefix = String::new();
                    for part in name.split('/') {
                        if !prefix.is_empty() {
                            prefix.push('/');
                        }
                        prefix.push_str(part);
                        candidates.insert(prefix.clone());
                    }
                }
            }
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                eprintln!("FILEMANIFEST_STREAM_25_STARTED {} {engine}", vector.id);
                let execution = if engine == "reference" {
                    hirsel_engine::run_observed_snapshot(&mut loaded.engine, &config, world.clone())
                } else {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world.clone())
                }
                .unwrap();
                assert!(
                    matches!(
                        execution.outcome,
                        Ok(hirsel_engine::RunOutcome::Returned(_))
                    ),
                    "{} {engine}: {:?}",
                    vector.id,
                    execution.outcome
                );
                let observation = execution.observation.unwrap();
                assert_eq!(
                    observation.exit_code,
                    Some(expected["status"].as_i64().unwrap() as i32)
                );
                assert_eq!(
                    observation.output,
                    expected["stdout"].as_str().unwrap().as_bytes()
                );
                assert_eq!(
                    observation.final_state.as_ref().unwrap().stderr,
                    expected["stderr"].as_str().unwrap().as_bytes()
                );
                assert!(
                    observation.failures.is_empty()
                        && observation.blocked.is_none()
                        && observation.signaled.is_none()
                );
                let mut reads = vec![];
                let mut opens = vec![];
                let mut metadata = vec![];
                let mut file_reads = 0;
                for effect in &observation.effects {
                    if let Effect::CapabilityConsumed(event) = &effect.effect {
                        match event.operation.as_str() {
                            "file.walk" => reads.push(String::new()),
                            "file.read_dir" => reads.push(event_path(&event.input, &candidates)),
                            "file.open" => opens.push(event_path(&event.input, &candidates)),
                            "file.metadata" => metadata.push(event_path(&event.input, &candidates)),
                            "file.read" => file_reads += 1,
                            "file.create" | "file.write" | "file.remove" | "file.write_bytes" => {
                                panic!("unexpected write: {event:?}")
                            }
                            _ => (),
                        }
                    }
                }
                let projection = json!({"read_dirs":reads,"opens":opens,"metadata":metadata,"file_reads":file_reads});
                for key in ["read_dirs", "opens", "metadata", "file_reads"] {
                    assert_eq!(
                        projection[key], expected[key],
                        "{} {engine}: {projection}",
                        vector.id
                    );
                }
                assert_eq!(
                    &observation.final_state.as_ref().unwrap().trees[&0].1,
                    &world.files
                );
                receipt(&vector, engine, &projection);
                if let Some(previous) = previous {
                    assert_eq!(observation, previous);
                }
                previous = Some(observation);
            }
        }
        fs::remove_dir_all(root).unwrap();
    });
}
fn native(caps: bool, backend: &str) {
    let root = scratch();
    package(&root);
    fs::write(
        root.join("manifest.json"),
        include_str!("../support/filemanifest_stream_25/manifest.json"),
    )
    .unwrap();
    fs::write(
        root.join("verify.py"),
        include_str!("../support/filemanifest_stream_25/verify.py"),
    )
    .unwrap();
    for vector in vectors(caps) {
        check_hashes(&vector);
        fs::write(root.join(format!("{}.input.json", vector.id)), vector.input).unwrap();
        fs::write(
            root.join(format!("{}.expected.json", vector.id)),
            vector.expected,
        )
        .unwrap();
    }
    let output = Command::new("python3")
        .arg(root.join("verify.py"))
        .arg("--root")
        .arg(&root)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--backend")
        .arg(backend)
        .arg("--partition")
        .arg(if caps { "caps" } else { "lazy" })
        .output()
        .unwrap();
    fs::write(root.join("stdout"), &output.stdout).unwrap();
    fs::write(root.join("stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "evidence {}: {}\n{}",
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for vector in vectors(caps) {
        let expected: Value = serde_json::from_str(vector.expected).unwrap();
        let bytes = fs::read(root.join(format!("evidence/{}.tape", vector.id))).unwrap();
        let (record, binary) = hirsel_rt::tape_binary::replay(bytes).unwrap();
        let events = binary.map(|t| t.events().unwrap()).unwrap_or(record.events);
        let mut reads = vec![];
        let mut opens = vec![];
        let mut metadata = vec![];
        let mut file_reads = 0;
        for event in events {
            use hirsel_rt::io::Request;
            match event.request {
                Request::ReadDir { path, .. } => reads.push(path),
                Request::Open { path, .. } => opens.push(path),
                Request::Metadata { path, .. } => metadata.push(path),
                Request::FileRead { .. } => file_reads += 1,
                _ => (),
            }
        }
        let projection =
            json!({"read_dirs":reads,"opens":opens,"metadata":metadata,"file_reads":file_reads});
        for key in ["read_dirs", "opens", "metadata", "file_reads"] {
            assert_eq!(
                projection[key],
                expected[key],
                "{} {backend}: {projection}; evidence {}",
                vector.id,
                root.display()
            );
        }
        receipt(&vector, backend, &projection);
    }
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn reference_checked_caps() {
    simulated(true);
}
#[test]
fn reference_checked_lazy() {
    simulated(false);
}
#[test]
fn llvm_caps() {
    native(true, "llvm");
}
#[test]
fn llvm_lazy() {
    native(false, "llvm");
}
#[test]
fn cranelift_caps() {
    native(true, "cranelift");
}
#[test]
fn cranelift_lazy() {
    native(false, "cranelift");
}
