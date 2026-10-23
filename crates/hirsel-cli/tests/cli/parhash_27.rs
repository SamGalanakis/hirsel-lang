//! Bounded application workflows; expected bytes are sealed host-oracle fixtures.
use hirsel_engine::{RunConfiguration, SimulationConfig};
use hirsel_interp::observation::Effect;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn data() -> Value {
    serde_json::from_str(include_str!("../support/parhash_27/cases.json")).unwrap()
}
fn scratch(label: &str) -> PathBuf {
    let evidence = std::env::var_os("PARHASH_27_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    tempfile::Builder::new()
        .prefix(&format!("parhash-27-{label}-"))
        .tempdir_in(evidence)
        .unwrap()
        .keep()
}
fn package(root: &Path) -> PathBuf {
    for (name, bytes) in [
        (
            "main.hir",
            include_str!("../../../../examples/parhash/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/parhash/model.hir"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
    ] {
        fs::write(root.join(name), bytes).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/parhash/hirsel.json")).unwrap();
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
fn receipt(row: &Value, engine: &str, extra: Value) {
    use sha2::{Digest, Sha256};
    eprintln!(
        "PARHASH_27 {}",
        json!({"id":row["id"],"engine":engine,"started":1,"completed":1,"passed":1,
        "source_sha256":format!("{:x}",Sha256::digest(include_bytes!("../../../../examples/parhash/main.hir"))),
        "cases_sha256":format!("{:x}",Sha256::digest(include_bytes!("../support/parhash_27/cases.json"))),
        "compiler":hirsel_driver::COMPILER_IDENTITY,"extra":extra})
    );
}
fn simulated(part: bool, only: Option<&str>) {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("simulation");
        let selection = package(&root);
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let data = data();
        for row in data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| (r["status"] == 0) == part && only.is_none_or(|id| r["id"] == id))
        {
            let world = SimulationConfig {
                args: serde_json::from_value(row["args"].clone()).unwrap(),
                mounts: [("fixtures".into(), hirsel_run_schema::Access::Read)].into(),
                projections: ["stdout", "stderr", "dir", "tasks"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                files: data["files"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(p, b)| {
                        (
                            format!("fixtures/{p}"),
                            b.as_str().unwrap().as_bytes().to_vec(),
                        )
                    })
                    .collect(),
                ..Default::default()
            };
            let mut world = world;
            for name in world.files.clone().keys() {
                let mut path = Path::new(name).parent();
                while let Some(p) = path {
                    world.directories.insert(p.to_str().unwrap().into());
                    path = p.parent();
                }
            }
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                eprintln!("PARHASH_27_STARTED {} {engine}", row["id"]);
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
                    row["id"],
                    execution.outcome
                );
                let observed = execution.observation.unwrap();
                assert_eq!(
                    observed.exit_code,
                    Some(row["status"].as_i64().unwrap() as i32),
                    "{} {engine}",
                    row["id"]
                );
                assert_eq!(
                    observed.output,
                    row["stdout"].as_str().unwrap().as_bytes(),
                    "{} {engine}",
                    row["id"]
                );
                let state = observed.final_state.as_ref().unwrap();
                assert_eq!(
                    state.stderr,
                    row["stderr"].as_str().unwrap().as_bytes(),
                    "{} {engine}",
                    row["id"]
                );
                assert!(
                    observed.failures.is_empty()
                        && observed.blocked.is_none()
                        && observed.signaled.is_none()
                );
                assert_eq!(&state.trees[&0].1, &world.files);
                // Outstanding unjoined handles are an upper bound on live jobs.
                let mut outstanding = 0;
                let mut maximum = 0;
                let mut spawned = 0;
                let mut closed = 0;
                let mut completion_order = vec![];
                for effect in &observed.effects {
                    match &effect.effect {
                        Effect::CapabilityConsumed(event) => match event.operation.as_str() {
                            "task.await_any.complete" => {
                                completion_order.push(u64::from_le_bytes(
                                    event.output.as_slice().try_into().unwrap(),
                                ));
                            }
                            "task.async" => {
                                outstanding += 1;
                                spawned += 1;
                                maximum = maximum.max(outstanding);
                            }
                            "task.await.complete" => {
                                assert!(outstanding > 0);
                                outstanding -= 1;
                            }
                            "task.cancel_all" => outstanding = 0,
                            "file.create" | "file.write" | "file.remove" | "file.walk"
                            | "file.read_dir" => panic!("unexpected effect {event:?}"),
                            _ => (),
                        },
                        Effect::HandleDropped { kind, .. } if kind == "File" => closed += 1,
                        _ => (),
                    }
                }
                assert_eq!(outstanding, 0);
                assert!(maximum <= row["jobs"].as_u64().unwrap());
                if row["status"] == 0 {
                    assert_eq!(spawned, row["children"].as_u64().unwrap());
                }
                if row["children"] == 0 {
                    assert_eq!(spawned, 0);
                }
                if only.is_some() {
                    assert_eq!(closed, 4);
                    assert_eq!(completion_order.len(), 4);
                    assert_ne!(completion_order, [0, 1, 2, 3]);
                    assert_ne!(completion_order[0], 0);
                }
                receipt(
                    row,
                    engine,
                    json!({"maximum_unjoined":maximum,"spawned":spawned,"file_drops":closed,"completion_order":completion_order}),
                );
                if let Some(previous) = previous {
                    assert_eq!(observed, previous);
                }
                previous = Some(observed);
            }
        }
        fs::remove_dir_all(root).unwrap();
    });
}
#[test]
fn reference_checked_success() {
    simulated(true, None);
}
#[test]
fn reference_checked_failures() {
    simulated(false, None);
}
fn native(backend: &str) {
    let root = scratch(backend);
    package(&root);
    fs::write(
        root.join("cases.json"),
        include_str!("../support/parhash_27/cases.json"),
    )
    .unwrap();
    fs::write(
        root.join("verify.py"),
        include_str!("../support/parhash_27/verify.py"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(root.join("verify.py"))
        .arg("--root")
        .arg(&root)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--backend")
        .arg(backend)
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
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    // Full recorded payloads remain compared in their original order.
    for row in data()["cases"].as_array().unwrap() {
        let tape = root.join(format!("evidence/{}.tape", row["id"].as_str().unwrap()));
        let recording = hirsel_rt::host_launch::read_recording(&tape).unwrap();
        assert!(!recording.events.is_empty());
        receipt(row, backend, json!({"events":recording.events.len()}));
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn llvm_workflows() {
    native("llvm");
}
#[test]
fn cranelift_workflows() {
    native("cranelift");
}
#[test]
fn digest_vectors_and_split_update() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("vectors");
        let selection = package(&root);
        let vectors: Value =
            serde_json::from_str(include_str!("../support/parhash_27/sha256.json")).unwrap();
        for vector in vectors.as_array().unwrap() {
            let mut source = String::from("pub fn main(t: Test) -> Result[(), ()] {\n");
            let input = vector["input"].as_str().unwrap();
            let half = input.len() / 2;
            source.push_str(&format!(
                r#"{{
                let data = b{whole}
                let expected = {expected}
                test.check(t, sha256.digest_hex(data) == expected)
                let mut state = sha256.new()
                sha256.update(mut state, b{first})
                sha256.update(mut state, b"")
                sha256.update(mut state, b{last})
                test.check(t, hex.encode(bytes.from_inline(sha256.finish(take state))) == expected)
            }}
"#,
                whole = serde_json::to_string(input).unwrap(),
                expected = vector["sha256"],
                first = serde_json::to_string(&input[..half]).unwrap(),
                last = serde_json::to_string(&input[half..]).unwrap()
            ));
            source.push_str("Ok(())\n}\n");
            fs::write(root.join("main.hir"), source).unwrap();
            let mut loaded = hirsel_driver::load_program(&selection).unwrap();
            loaded
                .check_for_execution()
                .unwrap_or_else(|error| panic!("{error}"));
            let config = RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            for checked in [false, true] {
                let execution = if checked {
                    hirsel_engine::run_observed_checked(
                        &mut loaded.engine,
                        &config,
                        SimulationConfig::default(),
                    )
                } else {
                    hirsel_engine::run_observed_snapshot(
                        &mut loaded.engine,
                        &config,
                        SimulationConfig::default(),
                    )
                }
                .unwrap();
                assert!(
                    matches!(
                        execution.outcome,
                        Ok(hirsel_engine::RunOutcome::Returned(_))
                    ),
                    "{:?}",
                    execution.outcome
                );
                assert!(execution.observation.unwrap().failures.is_empty());
            }
            eprintln!(
                "PARHASH_27_VECTOR {} reference+checked-h passed",
                vector["input"]
            );
        }
        fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn varied_completion_and_cleanup() {
    simulated(true, Some("varied"));
}

// Measurement is separate from production/timing: the public adapter selects
// the existing measurement runtime, with no engine or runtime implementation here.
#[test]
fn measured_copy_cost() {
    use hirsel_driver::native::{
        self, OptLevel, Options, Partition, RuntimeBuild, TargetSpec, Toolchain,
    };
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("copies");
        let selection = package(&root);
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let ready = native::build(&validated, &Options::new(target.clone())).unwrap();
        let tools = Toolchain::in_tree_with_runtime(
            &root.join("runtime"),
            &[&target],
            RuntimeBuild {
                measurement: true,
                ..Default::default()
            },
        )
        .unwrap();
        let objects =
            native::emit_objects(&ready, OptLevel::Speed, Partition::Whole, true, None).unwrap();
        let executable =
            native::link_standalone_objects(&ready, &loaded.entry, &objects, &tools, &root)
                .unwrap();
        fs::create_dir(root.join("fixtures")).unwrap();
        fs::write(root.join("fixtures/abc"), b"abc").unwrap();
        let row = data()["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "duplicate")
            .unwrap()
            .clone();
        let grants = root.join("grants.json");
        fs::write(&grants, json!({"io":{"stdout":"stdout.bytes@1","stderr":"stderr.bytes@1","tasks":"tasks.cooperative@1",
            "dirs":[{"path":"fixtures","host":root.join("fixtures"),"access":"read"}]},"args":row["args"]}).to_string()).unwrap();
        let totals = root.join("totals");
        let mut command = Command::new(executable);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("HIRSEL_RT_") {
                command.env_remove(key);
            }
        }
        let output = command
            .env("HIRSEL_RT_MEASUREMENT", &totals)
            .arg("--hirsel-grants")
            .arg(grants)
            .args(
                row["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap()),
            )
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(output.stdout, row["stdout"].as_str().unwrap().as_bytes());
        assert!(output.stderr.is_empty(), "{output:?}");
        let report = fs::read_to_string(totals).unwrap();
        assert!(
            report.lines().any(|line| line == "live_bytes 0"),
            "{report}"
        );
        assert!(
            report.lines().any(|line| line.starts_with("copied_bytes ")),
            "{report}"
        );
        eprintln!("PARHASH_27_COPY_MEASUREMENT\n{report}");
        fs::remove_dir_all(root).unwrap();
    });
}
