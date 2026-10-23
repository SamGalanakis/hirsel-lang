//! Full configctl entry/package workflows. Oracles are literal cases and Python
//! tomllib, independent of the app's TOML parser and typed transition helpers.
use hirsel_interp::library::simulation::{Fault, FileError};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn package(root: &Path) -> PathBuf {
    for (name, source) in [
        (
            "main.hir",
            include_str!("../../../../examples/configctl/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/configctl/model.hir"),
        ),
        (
            "schema.hir",
            include_str!("../../../../examples/configctl/schema.hir"),
        ),
        (
            "migration.hir",
            include_str!("../../../../examples/configctl/migration.hir"),
        ),
        (
            "document.hir",
            include_str!("../../../../examples/configctl/document.hir"),
        ),
        (
            "batch.hir",
            include_str!("../../../../examples/configctl/batch.hir"),
        ),
        (
            "publish.hir",
            include_str!("../../../../examples/configctl/publish.hir"),
        ),
        (
            "cases.json",
            include_str!("../support/config_migrate_29/cases.json"),
        ),
        (
            "verify.py",
            include_str!("../support/config_migrate_29/verify.py"),
        ),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/configctl/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    fs::write(
        root.join("caps.json"),
        include_str!("../support/config_migrate_29/caps.json"),
    )
    .unwrap();
    let selection = root.join("hirsel.json");
    fs::write(&selection, manifest.to_string()).unwrap();
    selection
}
fn scratch(label: &str) -> tempfile::TempDir {
    let base = std::env::var_os("CONFIG_MIGRATE_29_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
}
fn selected(caps: bool) -> Vec<Value> {
    let mut cases: Vec<Value> =
        serde_json::from_str(include_str!("../support/config_migrate_29/cases.json")).unwrap();
    if caps {
        let vectors: Vec<Value> =
            serde_json::from_str(include_str!("../support/config_migrate_29/caps.json")).unwrap();
        let source = "version=3\nlog_level=\"info\"\nservice.endpoint=\"x\"\ntimeouts={connect_ms=1,request_ms=1}\nretry={attempts=0,policy=\"none\"}\n";
        cases = vectors.into_iter().map(|c| {
            let count = c["count"].as_u64().unwrap() as usize;
            let files: Vec<String> = (0..count).map(|i| {
                let size = if i + 1 == count { c["last_bytes"].as_u64().unwrap_or(c["bytes"].as_u64().unwrap()) } else { c["bytes"].as_u64().unwrap() } as usize;
                if size < source.len() { " ".repeat(size) } else { format!("{source}{}", " ".repeat(size-source.len())) }
            }).collect();
            serde_json::json!({"id":c["id"],"files":files,"target":3,"jobs":c["jobs"].as_u64().unwrap_or(4),"exit":c["exit"],"errors":vec![Value::Null;count],"preflight":c["exit"]==1})
        }).collect();
    }
    if let Ok(filter) = std::env::var("CONFIG_MIGRATE_29_CASE") {
        cases.retain(|c| filter.split(',').any(|id| c["id"] == id));
    }
    assert!(!cases.is_empty(), "zero selected cases");
    cases
}
fn scripted(engine: &str, caps: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch(&format!("config-migrate-29-{engine}-"));
        let root = scratch.path();
        let selection = package(root);
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let cases = selected(caps);
        let mut incomplete = 0;
        for case in &cases {
            let id = case["id"].as_str().unwrap();
            eprintln!("CONFIG_MIGRATE_29_STARTED {engine}/{id}");
            let mut world = hirsel_engine::SimulationConfig::default();
            world.args = vec![
                case["command"].as_str().unwrap_or("toml-migrate").into(),
                "--to".into(),
                case["target"].to_string(),
                "--jobs".into(),
                case["jobs"].to_string(),
            ];
            world.mounts = [
                ("configs".into(), hirsel_run_schema::Access::Read),
                ("migrated".into(), hirsel_run_schema::Access::ReadWrite),
            ]
            .into();
            world
                .directories
                .extend(["configs".into(), "migrated".into()]);
            for (i, file) in case["files"].as_array().unwrap().iter().enumerate() {
                world.args.push(format!("{i}.toml"));
                if let Some(text) = file.as_str() {
                    world
                        .files
                        .insert(format!("configs/{i}.toml"), text.as_bytes().to_vec());
                }
            }
            if case["deny"] == true {
                world.projections.remove("tasks");
            }
            if let Some(operation) = case["fault"].as_str() {
                world.faults.insert(
                    operation.into(),
                    [if case["cancel"] == true {
                        Fault::Canceled(0)
                    } else if operation.starts_with("out.") {
                        Fault::Fault {
                            code: 77,
                            partial: 0,
                        }
                    } else {
                        Fault::File {
                            error: FileError::Fault(77),
                            partial: 0,
                        }
                    }]
                    .into(),
                );
            }
            let execution = if engine == "reference" {
                hirsel_engine::run_observed_snapshot(&mut loaded.engine, &config, world)
            } else {
                hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world)
            }
            .unwrap();
            let Some(observed) = execution.observation.as_ref() else {
                let detail = format!("{execution:?}");
                fs::write(root.join(format!("{id}-incomplete.txt")), &detail).unwrap();
                assert!(
                    detail.contains("ToolingLimit")
                        && detail.contains("Steps")
                        && detail.contains("1000000"),
                    "{engine}/{id}: {detail}"
                );
                eprintln!("CONFIG_MIGRATE_29_INCOMPLETE {engine}/{id}: {detail}");
                incomplete += 1;
                continue;
            };
            assert_eq!(
                observed.exit_code,
                case["exit"].as_i64().map(|n| n as i32),
                "{engine}/{id}: {execution:?}"
            );
            assert!(
                observed.failures.is_empty()
                    && observed.blocked.is_none()
                    && observed.signaled.is_none(),
                "{engine}/{id}: {observed:?}"
            );
            let directory = root.join(id);
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join("stdout"), &observed.output).unwrap();
            let state = observed.final_state.as_ref().unwrap();
            for (_, files) in state.trees.values() {
                for (path, bytes) in files {
                    if let Some(name) = path.strip_prefix("migrated/") {
                        fs::write(directory.join(name), bytes).unwrap();
                    }
                    assert!(!path.contains(".tmp"), "atomic temporary leaked: {path}");
                }
            }
            if caps {
                if case["preflight"] == true {
                    assert!(observed.output.is_empty());
                    assert!(!directory.join("manifest.json").exists());
                } else {
                    for (i, source) in case["files"].as_array().unwrap().iter().enumerate() {
                        assert_eq!(
                            fs::read(directory.join(format!("{i}.toml"))).unwrap(),
                            source.as_str().unwrap().as_bytes()
                        );
                    }
                }
                eprintln!("CONFIG_MIGRATE_29_PASS {engine}/{id}");
                continue;
            }
            let mut oracle_command = Command::new("python3");
            if case["command"] == "toml-plan" {
                oracle_command.arg(root.join("verify.py")).arg("--plan");
            } else {
                oracle_command.arg(root.join("verify.py"));
            }
            let oracle = oracle_command
                .arg("--oracle")
                .arg(id)
                .arg("--root")
                .arg(root)
                .output()
                .unwrap();
            assert!(
                oracle.status.success(),
                "{engine}/{id}: {} {}",
                String::from_utf8_lossy(&oracle.stdout),
                String::from_utf8_lossy(&oracle.stderr)
            );
            eprintln!("CONFIG_MIGRATE_29_PASS {engine}/{id}");
        }
        eprintln!(
            "CONFIG_MIGRATE_29_COMPLETED {engine} selected={} semantic_pass={} incomplete={incomplete}",
            cases.len(),
            cases.len() - incomplete
        );
        if std::env::var_os("CONFIG_MIGRATE_29_EVIDENCE_DIR").is_some() {
            let _ = scratch.keep();
        }
    });
}
#[test]
fn config_migrate_29_reference() {
    scripted("reference", false);
}
#[test]
fn config_migrate_29_checked_h() {
    scripted("checked-h", false);
}
fn native(backend: &str) {
    let scratch = scratch(&format!("config-migrate-29-{backend}-"));
    package(scratch.path());
    fs::write(
        scratch.path().join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(scratch.path().join("verify.py"))
        .arg("--native")
        .arg(backend)
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(scratch.path())
        .output()
        .unwrap();
    fs::write(scratch.path().join("driver.stdout"), &output.stdout).unwrap();
    fs::write(scratch.path().join("driver.stderr"), &output.stderr).unwrap();
    let evidence = scratch.keep();
    assert!(
        output.status.success(),
        "{}: {} {}",
        evidence.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let tapes: Vec<Value> =
        serde_json::from_slice(&fs::read(evidence.join("tapes.json")).unwrap()).unwrap();
    let mut prior = std::collections::BTreeMap::new();
    for cell in &tapes {
        let mut recording =
            hirsel_rt::host_launch::read_recording(Path::new(cell["path"].as_str().unwrap()))
                .unwrap();
        let binding = recording.binding.as_mut().unwrap();
        binding.admitted_at = 0;
        binding.placement = None;
        let key = format!("{}/{}", cell["backend"], cell["case"]);
        if let Some(old) = prior.get(&key) {
            assert_eq!(
                old, &recording,
                "semantic event order/payload across widths: {cell}"
            );
        } else {
            prior.insert(key, recording);
        }
    }
    // Tamper the bound input/config independently of CRC framing, retaining
    // all original events. The supported replay readers must refuse both.
    let original =
        hirsel_rt::host_launch::read_recording(Path::new(tapes[0]["path"].as_str().unwrap()))
            .unwrap();
    for kind in ["arguments", "config"] {
        let mut altered = original.clone();
        if kind == "arguments" {
            altered.arguments.push("changed".into());
        } else {
            let binding = altered.binding.as_mut().unwrap();
            let mut config: Value = serde_json::from_slice(&binding.canonical).unwrap();
            config["args"] = serde_json::json!(["changed"]);
            binding.canonical = serde_json::to_vec(&config).unwrap();
        }
        let events = std::mem::take(&mut altered.events);
        let tape = evidence.join(format!("changed-{kind}.tape"));
        let mut writer =
            hirsel_rt::tape_binary::Writer::new(fs::File::create(&tape).unwrap(), &altered)
                .unwrap();
        for event in &events {
            writer.append(event).unwrap();
        }
        writer.finish().unwrap();
        drop(writer);
        for engine in ["reference", "llvm", "cranelift"] {
            let mut command = if engine == "reference" {
                let mut command = Command::new(env!("CARGO_BIN_EXE_hirsel"));
                command
                    .arg("run")
                    .arg(evidence.join("hirsel.json"))
                    .arg("--replay");
                command
            } else {
                let mut command = Command::new(evidence.join(format!("app-{engine}")));
                command.arg("--hirsel-replay");
                command
            };
            let refused = command.arg(&tape).output().unwrap();
            assert_eq!(
                refused.status.code(),
                Some(3),
                "{kind}/{engine}: {refused:?}"
            );
            assert!(refused.stdout.is_empty());
        }
    }
    eprintln!(
        "CONFIG_MIGRATE_29_NATIVE {backend}: {} evidence={}",
        String::from_utf8_lossy(&output.stdout),
        evidence.display()
    );
}
#[test]
fn config_migrate_29_native() {
    native("both");
}

#[test]
fn config_migrate_29_bounds_reference() {
    scripted("reference", true);
}
#[test]
fn config_migrate_29_bounds_checked_h() {
    scripted("checked-h", true);
}
