//! Full package entry; pure or surrogate models cannot satisfy these checks.
use hirsel_engine::{RunConfiguration, SimulationConfig};
use hirsel_interp::observation::Effect;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn package(root: &Path) -> PathBuf {
    for (name, source) in [
        ("main", include_str!("../../../../examples/mdman/main.hir")),
        (
            "manifest",
            include_str!("../../../../examples/mdman/manifest.hir"),
        ),
        (
            "document",
            include_str!("../../../../examples/mdman/document.hir"),
        ),
        ("roff", include_str!("../../../../examples/mdman/roff.hir")),
        (
            "batch",
            include_str!("../../../../examples/mdman/batch.hir"),
        ),
        (
            "publish",
            include_str!("../../../../examples/mdman/publish.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/mdman/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    let selection = root.join("hirsel.json");
    fs::write(&selection, manifest.to_string()).unwrap();
    selection
}
fn bytes(value: &Value) -> Vec<u8> {
    value.as_str().map_or_else(
        || {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect()
        },
        |s| s.as_bytes().to_vec(),
    )
}
fn cases() -> Vec<Value> {
    let output = Command::new("python3")
        .arg("-c")
        .arg(include_str!("../support/man_render_30/vectors.py"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let mut rows: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    if let Ok(filter) = std::env::var("MAN_RENDER_30_CASE") {
        rows.retain(|r| filter.split(',').any(|s| r["id"] == s));
    }
    assert!(!rows.is_empty(), "zero selected cases");
    rows
}
fn scratch(label: &str) -> tempfile::TempDir {
    let root = std::env::var_os("MAN_RENDER_30_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(root)
        .unwrap()
}
fn scripted(selected: Vec<Value>) {
    hirsel_engine::with_compiler_stack(|| {
        let tmp = scratch("man-script-");
        let selection = package(tmp.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let rows = selected;
        let mut completed = 0;
        let mut incomplete = 0;
        for row in &rows {
            let id = row["id"].as_str().unwrap();
            let mut world = SimulationConfig {
                args: serde_json::from_value(row["args"].clone()).unwrap(),
                mounts: [(
                    "data".into(),
                    if row["read_only"] == true {
                        hirsel_run_schema::Access::Read
                    } else {
                        hirsel_run_schema::Access::ReadWrite
                    },
                )]
                .into(),
                ..Default::default()
            };
            world.directories.insert("data".into());
            for (name, data) in row["files"].as_object().unwrap() {
                world.files.insert(format!("data/{name}"), bytes(data));
            }
            if let Some(name) = row["pending"].as_str() {
                world
                    .file_reads
                    .insert(name.into(), hirsel_run_schema::FileReadScript::Pending);
            }
            if let Some(seed) = row["scheduler_seed"].as_u64() {
                world.scheduler = Some(hirsel_run_schema::Scheduler {
                    algorithm: "random".into(),
                    version: 1,
                    seed,
                });
            }
            if row["commit_fault"] == true {
                use hirsel_interp::library::simulation::{Fault, FileError};
                world.faults.insert(
                    "file.commit".into(),
                    [Fault::File {
                        error: FileError::PermissionDenied,
                        partial: 0,
                    }]
                    .into(),
                );
            }
            let mut previous = None;
            for checked in [false, true] {
                let engine = if checked { "checked-h" } else { "reference" };
                eprintln!("MAN_RENDER_30_STARTED {id}/{engine}");
                let bound = row["steps"].as_u64().unwrap();
                let result = if checked {
                    hirsel_engine::run_observed_checked_bounded(
                        &mut loaded.engine,
                        &run,
                        world.clone(),
                        bound,
                    )
                } else {
                    hirsel_engine::run_observed_snapshot_bounded(
                        &mut loaded.engine,
                        &run,
                        world.clone(),
                        bound,
                    )
                }
                .unwrap();
                eprintln!("MAN_RENDER_30_STEPS {id}/{engine} {:?}", result.step_usage);
                let Some(observation) = result.observation.as_ref() else {
                    assert!(
                        matches!(
                            result.outcome,
                            Ok(hirsel_engine::RunOutcome::ToolingLimit(ref limit)) if limit.kind == "Steps" && limit.limit == bound
                        ),
                        "{id}/{engine}: {result:?}"
                    );
                    eprintln!(
                        "MAN_RENDER_30_INCOMPLETE {id}/{engine} {:?}",
                        result.outcome
                    );
                    incomplete += 1;
                    continue;
                };
                assert_eq!(
                    observation.exit_code,
                    Some(row["exit"].as_i64().unwrap() as i32),
                    "{id}/{engine}: {observation:?}"
                );
                assert_eq!(observation.output, bytes(&row["stdout"]), "{id}/{engine}");
                assert!(
                    observation.failures.is_empty()
                        && observation.blocked.is_none()
                        && observation.signaled.is_none(),
                    "{id}/{engine}: {observation:?}"
                );
                let tree = &observation.final_state.as_ref().unwrap().trees[&0].1;
                for (path, expected) in row["outputs"].as_object().unwrap() {
                    assert_eq!(
                        tree[&format!("data/{path}")],
                        bytes(expected),
                        "{id}/{engine}/{path}"
                    );
                }
                assert_eq!(
                    tree.len(),
                    row["files"].as_object().unwrap().len()
                        + row["outputs"].as_object().unwrap().len(),
                    "{id}: unexpected effects {tree:?}"
                );
                if row["pending"].is_string() {
                    assert!(observation.effects.iter().any(|e|matches!(&e.effect,Effect::CapabilityConsumed(e) if e.operation=="file.read.pending")),"pending read never parked");
                }
                if id == "pending-cancel-queued" {
                    assert!(!observation.effects.iter().any(|effect|matches!(&effect.effect,Effect::CapabilityConsumed(e) if e.operation=="file.open" && e.input.windows(4).any(|w|w==b"e.md"))),"queued fifth page opened");
                    assert!(observation.effects.iter().any(|effect|matches!(&effect.effect,Effect::HandleDropped{kind,..} if kind.contains("File"))),"file cleanup absent");
                }
                if let Some(prior) = previous {
                    assert_eq!(*observation, prior, "{id}: semantic order");
                }
                previous = Some(observation.clone());
                completed += 1;
                eprintln!("MAN_RENDER_30_PASS {id}/{engine}");
            }
        }
        assert_eq!(completed + incomplete, rows.len() * 2);
        eprintln!(
            "MAN_RENDER_30_COUNTS selected={} completed={completed} pass={completed} incomplete={incomplete}",
            rows.len() * 2
        );
        if std::env::var_os("MAN_RENDER_30_EVIDENCE").is_some() {
            let _ = tmp.keep();
        }
    });
}
#[test]
fn man_render_30_scripted_reference_checked() {
    scripted(cases())
}
#[test]
fn man_render_30_additional_scripted() {
    let selected = cases()
        .into_iter()
        .filter(|r| {
            [
                "pending-cancel-queued",
                "publication-continues",
                "reordered-batch",
            ]
            .iter()
            .any(|id| r["id"] == *id)
        })
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 3);
    scripted(selected)
}
fn native(backend: &str) {
    let tmp = scratch(&format!("man-{backend}-"));
    let retained_root = tmp.keep();
    let root = retained_root.as_path();
    let selection = package(root);
    fs::write(
        root.join("vectors.py"),
        include_str!("../support/man_render_30/vectors.py"),
    )
    .unwrap();
    fs::write(
        root.join("native.py"),
        include_str!("../support/man_render_30/native.py"),
    )
    .unwrap();
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let result = Command::new("python3")
        .arg(root.join("native.py"))
        .args([
            "--hirsel",
            env!("CARGO_BIN_EXE_hirsel"),
            "--backend",
            backend,
        ])
        .arg("--root")
        .arg(root)
        .output()
        .unwrap();
    fs::write(root.join("stdout.log"), &result.stdout).unwrap();
    fs::write(root.join("stderr.log"), &result.stderr).unwrap();
    if !result.status.success() {
        let retained = &retained_root;
        panic!(
            "evidence {}: {} {}",
            retained.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    eprintln!("{}", String::from_utf8_lossy(&result.stdout));
    hirsel_engine::with_compiler_stack(|| {
        for path in fs::read_dir(root)
            .unwrap()
            .map(|p| p.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "tape"))
        {
            eprintln!(
                "MAN_RENDER_30_STARTED {} reference+checked-host-replay bound=10000000",
                path.display()
            );
            let ordinary = hirsel_driver::replay_observations(&selection, &path, Some(10_000_000));
            let checked =
                hirsel_driver::replay_observations_checked(&selection, &path, Some(10_000_000));
            let large = [
                "source16384",
                "total131072",
                "nodes8192",
                "nodes8193",
                "output65536",
                "output65537",
                "depth32",
                "depth33",
            ]
            .iter()
            .any(|id| path.file_name().unwrap().to_string_lossy().starts_with(id));
            if large
                && ordinary.as_ref().err().is_some_and(|e| {
                    e.to_string()
                        .contains("replay produced no semantic observation")
                })
            {
                assert!(checked.as_ref().err().is_some_and(|e| {
                    e.to_string()
                        .contains("replay produced no semantic observation")
                }));
                eprintln!(
                    "MAN_RENDER_30_INCOMPLETE {} reference+checked-host-replay no observation at fixed10000000",
                    path.display()
                );
                continue;
            }
            let ordinary = ordinary.unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let checked = checked.unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(ordinary.equivalent(&checked), "{}", path.display());
            let expected: Value =
                serde_json::from_slice(&fs::read(path.with_extension("run.json")).unwrap())
                    .unwrap();
            assert_eq!(
                ordinary.output,
                bytes(&expected["stdout"]),
                "{}",
                path.display()
            );
            assert_eq!(
                ordinary.exit_code,
                Some(expected["exit"].as_i64().unwrap() as i32)
            );
            assert!(ordinary.failures.is_empty() && ordinary.blocked.is_none());
            eprintln!(
                "MAN_RENDER_30_PASS {} reference+checked-host-replay",
                path.display()
            );
        }
    });
}
#[test]
fn man_render_30_native_llvm() {
    native("llvm")
}
#[test]
fn man_render_30_native_cranelift() {
    native("cranelift")
}

// Refusal cells use a new host-recorded metadata-error workflow. Existing native
// success cells are not repeated to replace missing archival evidence.
#[test]
fn man_render_30_host_refusal_controls() {
    hirsel_engine::with_compiler_stack(|| {
        let root = scratch("man-controls-").keep();
        let selection = package(&root);
        let data = root.join("data");
        fs::create_dir(&data).unwrap();
        let grants = root.join("grants.json");
        fs::write(&grants, serde_json::json!({"args":["check","in.md","TEST","0"],"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[{"path":"data","host":data,"access":"read"}]}}).to_string()).unwrap();
        let tape = root.join("original.tape");
        let recorded = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(&selection)
            .arg("--grants")
            .arg(&grants)
            .arg("--record")
            .arg(&tape)
            .output()
            .unwrap();
        assert_eq!(recorded.status.code(), Some(2), "{recorded:?}");
        assert_eq!(recorded.stdout, b"in.md: invalid section (1..9)\n");
        assert!(recorded.stderr.is_empty(), "{recorded:?}");
        let original = fs::read(&tape).unwrap();
        fs::remove_dir(&data).unwrap();
        fs::remove_file(&grants).unwrap();
        let mut binaries = vec![];
        for backend in ["llvm", "cranelift"] {
            let binary = root.join(backend);
            let build = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["build", "--backend", backend])
                .arg(&selection)
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            fs::write(root.join(format!("build-{backend}.log")), &build.stderr).unwrap();
            assert!(build.status.success(), "{build:?}");
            binaries.push(binary);
        }
        let recording = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
        let mut variants = vec![];
        let mut changed = recording.clone();
        for event in &mut changed.events {
            if let hirsel_rt::io::Request::Directory { path, .. } = &mut event.request {
                path.push_str("-changed");
                break;
            }
        }
        variants.push(("input", changed));
        let mut extra = recording.clone();
        extra.events.push(recording.events[0].clone());
        variants.push(("extra", extra));
        let mut config = recording.clone();
        let binding = config.binding.as_mut().unwrap();
        let canonical = String::from_utf8(binding.canonical.clone()).unwrap();
        let changed = canonical.replacen("\"check\"", "\"render\"", 1);
        assert_ne!(canonical, changed);
        binding.canonical = changed.into_bytes();
        variants.push(("config", config));
        let capped = root.join("capped");
        fs::write(&capped, include_str!("../../../../tools/capped")).unwrap();
        let mut controls = 0;
        let only = std::env::var("MAN_RENDER_30_CONTROL").ok();
        for (label, variant) in variants {
            if only.as_deref().is_some_and(|wanted| wanted != label) {
                continue;
            }
            let bad = root.join(format!("{label}.bad"));
            fs::write(&bad, hirsel_rt::launch::render_recording(&variant).unwrap()).unwrap();
            for checked in [false, true] {
                let result = if checked {
                    hirsel_driver::replay_observations_checked(&selection, &bad, Some(1_000_000))
                } else {
                    hirsel_driver::replay_observations(&selection, &bad, Some(1_000_000))
                };
                assert!(result.is_err(), "{label}/{checked}: accepted changed tape");
                controls += 1;
                eprintln!(
                    "MAN_RENDER_30_REFUSAL_PASS {label} checked={checked}: {}",
                    result.unwrap_err()
                );
            }
            for binary in &binaries {
                let result = Command::new("python3")
                    .arg(&capped)
                    .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
                    .arg(binary)
                    .arg("--hirsel-replay")
                    .arg(&bad)
                    .env("HIRSEL_JOBS", "4")
                    .env("HIRSEL_MEMORY_LIMIT", "2G")
                    .output()
                    .unwrap();
                fs::write(
                    root.join(format!(
                        "{label}-{}.log",
                        binary.file_name().unwrap().to_string_lossy()
                    )),
                    &result.stderr,
                )
                .unwrap();
                assert_eq!(result.status.code(), Some(3), "{label}: {result:?}");
                controls += 1;
                eprintln!("MAN_RENDER_30_REFUSAL_PASS {label} {}", binary.display());
            }
        }
        if only.as_deref().is_none_or(|wanted| wanted == "source") {
            let source = root.join("main.hir");
            let baseline = fs::read_to_string(&source).unwrap();
            fs::write(
                &source,
                baseline.replace("path.of(\"data\")", "path.of(\"changed\")"),
            )
            .unwrap();
            for checked in [false, true] {
                let result = if checked {
                    hirsel_driver::replay_observations_checked(&selection, &tape, Some(1_000_000))
                } else {
                    hirsel_driver::replay_observations(&selection, &tape, Some(1_000_000))
                };
                assert!(result.is_err(), "accepted changed source");
                controls += 1;
                eprintln!(
                    "MAN_RENDER_30_REFUSAL_PASS source checked={checked}: {}",
                    result.unwrap_err()
                );
            }
            for backend in ["llvm", "cranelift"] {
                let binary = root.join(format!("changed-{backend}"));
                let build = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                    .args(["build", "--backend", backend])
                    .arg(&selection)
                    .arg("-o")
                    .arg(&binary)
                    .output()
                    .unwrap();
                assert!(build.status.success(), "{build:?}");
                let result = Command::new("python3")
                    .arg(&capped)
                    .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
                    .arg(&binary)
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .env("HIRSEL_JOBS", "4")
                    .env("HIRSEL_MEMORY_LIMIT", "2G")
                    .output()
                    .unwrap();
                assert_eq!(result.status.code(), Some(3), "{result:?}");
                controls += 1;
                eprintln!("MAN_RENDER_30_REFUSAL_PASS source {backend}");
            }
        }
        assert_eq!(controls, if only.is_some() { 4 } else { 16 });
        assert_eq!(fs::read(&tape).unwrap(), original);
        assert!(!data.exists());
        eprintln!(
            "MAN_RENDER_30_REFUSAL_COUNTS passed={controls} evidence={}",
            root.display()
        );
    });
}
