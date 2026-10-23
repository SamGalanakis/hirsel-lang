//! Journal lane: the append-open capability seam and complete application workflows.
use serde_json::json;
use std::{fs, path::PathBuf, process::Command};

struct Scratch(Option<tempfile::TempDir>);
impl Scratch {
    fn new() -> Self {
        let base = std::env::var_os("JOURNAL_KV_29_EVIDENCE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&base).unwrap();
        Self(Some(
            tempfile::Builder::new()
                .prefix("journal-kv-29-")
                .tempdir_in(base)
                .unwrap(),
        ))
    }
    fn path(&self) -> &std::path::Path {
        self.0.as_ref().unwrap().path()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if std::env::var_os("JOURNAL_KV_29_EVIDENCE_DIR").is_some() {
            eprintln!(
                "JOURNAL_KV_29_EVIDENCE {}",
                self.0.take().unwrap().keep().display()
            );
        }
    }
}

const APPEND: &str = include_str!("../support/journal_kv_29/open_append.hir");

fn append_fixture(root: &std::path::Path) -> PathBuf {
    let source = root.join("main.hir");
    fs::write(&source, APPEND).unwrap();
    let stdlib = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    fs::write(
        source.with_extension("hir.json"),
        json!({"schema":1,
            "package":{"name":"appendtest","version":"1"},
            "sources":[{"path":"main.hir","module":[]}],
            "entry":{"module":[],"name":"main"},
            "dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}]
        })
        .to_string(),
    )
    .unwrap();
    source
}

#[test]
fn journal_kv_29_open_append_reference_checked() {
    hirsel_engine::with_compiler_stack(|| {
        let root = Scratch::new();
        let mut loaded = hirsel_driver::load_program(&append_fixture(root.path())).unwrap();
        loaded.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for existing in [false, true] {
            let mut world = hirsel_engine::SimulationConfig::default();
            world.directories.insert("data".into());
            world
                .mounts
                .insert("data".into(), hirsel_run_schema::Access::ReadWrite);
            if existing {
                world.files.insert("data/log".into(), b"prefix".to_vec());
            }
            let expected = if existing {
                b"prefixfirstsecond".as_slice()
            } else {
                b"firstsecond".as_slice()
            };
            let mut previous = None;
            for checked in [false, true] {
                let execution = if checked {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world.clone())
                } else {
                    hirsel_engine::run_observed_snapshot(&mut loaded.engine, &config, world.clone())
                }
                .unwrap();
                let observation = execution.observation.as_ref().unwrap();
                assert_eq!(observation.exit_code, Some(0), "{execution:?}");
                assert_eq!(observation.output, expected);
                assert!(observation.failures.is_empty());
                if let Some(previous) = previous {
                    assert_eq!(*observation, previous);
                }
                previous = Some(observation.clone());
                eprintln!("JOURNAL_KV_29_PASS append existing={existing} checked={checked}");
            }
        }
    });
}

#[test]
fn journal_kv_29_open_append_native_replay() {
    let root = Scratch::new();
    let source = append_fixture(root.path());
    let cli = env!("CARGO_BIN_EXE_hirsel");
    fs::write(
        root.path().join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let mut apps = vec![];
    for backend in ["llvm", "cranelift"] {
        let app = root.path().join(backend);
        let built = Command::new(cli)
            .args(["build", "--backend", backend])
            .arg(&source)
            .arg("-o")
            .arg(&app)
            .output()
            .unwrap();
        assert!(built.status.success(), "{built:?}");
        apps.push(app);
    }
    for (backend, app) in ["llvm", "cranelift"].into_iter().zip(&apps) {
        let data = root.path().join(format!("data-{backend}"));
        fs::create_dir(&data).unwrap();
        fs::write(data.join("log"), b"prefix").unwrap();
        let grants = root.path().join(format!("grants-{backend}.json"));
        fs::write(&grants,json!({"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1", "dirs":[{"path":"data","host":data,"access":"read_write"}]}}).to_string()).unwrap();
        let tape = root.path().join(format!("{backend}.tape"));
        let run = Command::new("python3")
            .arg(root.path().join("capped"))
            .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
            .arg(app)
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .env("HIRSEL_JOBS", "4")
            .arg("--hirsel-record")
            .arg(&tape)
            .arg("--hirsel-grants")
            .arg(&grants)
            .output()
            .unwrap();
        assert_eq!(run.status.code(), Some(0), "{run:?}");
        assert_eq!(run.stdout, b"prefixfirstsecond");
        assert_eq!(fs::read(data.join("log")).unwrap(), run.stdout);
        assert!(!data.join("denied").exists());
        fs::remove_dir_all(&data).unwrap();
        fs::remove_file(grants).unwrap();
        let immutable = fs::read(&tape).unwrap();
        let interpreted = Command::new("python3")
            .arg(root.path().join("capped"))
            .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
            .arg(cli)
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .env("HIRSEL_JOBS", "4")
            .arg("run")
            .arg(&source)
            .arg("--replay")
            .arg(&tape)
            .output()
            .unwrap();
        assert_eq!(interpreted.status.code(), Some(0), "{interpreted:?}");
        assert_eq!(interpreted.stdout, run.stdout);
        for replay_app in &apps {
            let replay = Command::new("python3")
                .arg(root.path().join("capped"))
                .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
                .arg(replay_app)
                .env("HIRSEL_MEMORY_LIMIT", "2G")
                .env("HIRSEL_JOBS", "4")
                .arg("--hirsel-replay")
                .arg(&tape)
                .output()
                .unwrap();
            assert_eq!(replay.status.code(), Some(0), "{replay:?}");
            assert_eq!(replay.stdout, run.stdout);
            assert_eq!(fs::read(&tape).unwrap(), immutable);
            assert!(!data.exists());
        }
        eprintln!("JOURNAL_KV_29_PASS append {backend} + three replay adapters");
    }
    for backend in [None, Some("llvm"), Some("cranelift")] {
        let mut cmd = Command::new(cli);
        cmd.arg("test").arg(&source);
        if let Some(backend) = backend {
            cmd.args(["--native", "--backend", backend]);
        }
        let tests = cmd.output().unwrap();
        assert_eq!(tests.status.code(), Some(0), "{tests:?}");
        assert!(
            String::from_utf8_lossy(&tests.stdout).contains("2 passed"),
            "{tests:?}"
        );
    }
}

#[test]
fn journal_kv_29_open_append_crash_modes_checked() {
    hirsel_engine::with_compiler_stack(|| {
        let root = Scratch::new();
        let source = append_fixture(root.path());
        // The public execution adapter requires a public root entry. Keep the
        // native fixture unchanged and expose only this scratch test selection.
        fs::write(&source, APPEND.replace("@test\nfn ", "@test\npub fn ")).unwrap();
        let mut loaded = hirsel_driver::load_program(&source).unwrap();
        loaded.check_for_execution().unwrap();
        for name in ["crash_reopen", "create_still_truncates_and_path_refusals"] {
            let mut entry = loaded.entry.clone();
            entry.0.name = name.into();
            let config = hirsel_engine::RunConfiguration {
                entry,
                hash_seed: 0,
                inputs: vec![],
            };
            let mut world = hirsel_engine::SimulationConfig::default();
            world.grants.insert("Test".into());
            let execution =
                hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world).unwrap();
            let observation = execution.observation.as_ref().unwrap();
            assert_eq!(observation.exit_code, Some(0), "{name}: {execution:?}");
            assert!(observation.failures.is_empty(), "{name}: {execution:?}");
            eprintln!("JOURNAL_KV_29_PASS {name} checked-h");
        }
    });
}

fn app_package(root: &std::path::Path) -> PathBuf {
    for (name, source) in [
        (
            "main",
            include_str!("../../../../examples/journalkv/main.hir"),
        ),
        (
            "record",
            include_str!("../../../../examples/journalkv/record.hir"),
        ),
        (
            "recover",
            include_str!("../../../../examples/journalkv/recover.hir"),
        ),
        (
            "index",
            include_str!("../../../../examples/journalkv/index.hir"),
        ),
        (
            "compact",
            include_str!("../../../../examples/journalkv/compact.hir"),
        ),
        (
            "session",
            include_str!("../../../../examples/journalkv/session.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/journalkv/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    let path = root.join("hirsel.json");
    fs::write(&path, manifest.to_string()).unwrap();
    path
}
fn unhex(text: &str) -> Vec<u8> {
    assert!(text.len() % 2 == 0);
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}
fn fixture_bytes(value: &serde_json::Value) -> Vec<u8> {
    if let Some(text) = value.as_str() {
        return unhex(text);
    }
    let mut bytes = value["prefix"].as_str().map(unhex).unwrap_or_default();
    if let Some(frame) = value["frame"].as_str() {
        let frame = unhex(frame);
        let count = value["repeat"].as_u64().unwrap();
        assert!(count <= 257 && frame.len() <= 8199);
        for _ in 0..count {
            let remaining = 32768 - bytes.len() % 32768;
            if remaining < frame.len() {
                bytes.resize(bytes.len() + remaining, 0);
            }
            bytes.extend_from_slice(&frame);
        }
    } else {
        let count = value["count"].as_u64().unwrap() as usize;
        assert!(count <= 1048577);
        bytes.resize(
            bytes.len() + count,
            value["byte"].as_u64().unwrap().try_into().unwrap(),
        );
    }
    assert!(bytes.len() <= 1048577);
    bytes
}
fn cases() -> Vec<serde_json::Value> {
    serde_json::from_str(include_str!("../support/journal_kv_29/cases.json")).unwrap()
}
fn fixture_world(case: &serde_json::Value) -> hirsel_engine::SimulationConfig {
    let mut world = hirsel_engine::SimulationConfig::default();
    world.args = case["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().into())
        .collect();
    world.mounts = [
        ("commands".into(), hirsel_run_schema::Access::Read),
        ("data".into(), hirsel_run_schema::Access::ReadWrite),
    ]
    .into();
    world.directories.extend(["data".into(), "commands".into()]);
    world.projections = ["dir", "stdout", "tasks", "signals"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    world.files = case["files"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), fixture_bytes(v)))
        .collect();
    if let Some(faults) = case["fault"].as_object() {
        for (operation, value) in faults {
            let fault = if let Some(cause) = value["canceled"].as_u64() {
                hirsel_interp::library::simulation::Fault::Canceled(cause as u32)
            } else {
                hirsel_interp::library::simulation::Fault::Fault {
                    code: value["code"].as_u64().unwrap() as u32,
                    partial: value["partial"].as_u64().unwrap() as usize,
                }
            };
            world.faults.insert(operation.clone(), [fault].into());
        }
    }
    world
}
fn assert_error_class(
    observation: &hirsel_interp::observation::Observable,
    expected: Option<&str>,
) {
    use hirsel_interp::observation::{ObservedAggregate as A, ObservedNode as N};
    let returned = observation.returned.as_ref().unwrap();
    let N::Aggregate {
        kind: A::Variant(result),
        fields,
    } = &returned.nodes[0]
    else {
        panic!("{returned:?}")
    };
    assert_eq!(*result, if expected.is_some() { 1 } else { 0 });
    if let Some(name) = expected {
        let tags = [
            "Json", "Files", "Grant", "Output", "Canceled", "Limit", "Corrupt", "Torn", "Usage",
            "Signals",
        ];
        let N::Aggregate {
            kind: A::Variant(tag),
            ..
        } = &returned.nodes[fields[0].unwrap()]
        else {
            panic!("{returned:?}")
        };
        assert_eq!(tags[*tag as usize], name, "{returned:?}");
    }
}
#[test]
fn journal_kv_29_application_reference_checked() {
    reference_cases(cases(), false);
}
#[test]
#[ignore = "full gate: original application audit"]
fn full_gate_journal_kv_29_boundaries_reference_checked() {
    reference_cases(
        serde_json::from_str(include_str!("../support/journal_kv_29/caps.json")).unwrap(),
        true,
    );
}
fn reference_cases(mut selected: Vec<serde_json::Value>, boundaries: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let root = Scratch::new();
        let selection = app_package(root.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        if let Ok(filter) = std::env::var("JOURNAL_KV_29_CASE") {
            selected.retain(|c| filter.split(',').any(|id| c["id"] == id));
        }
        assert!(!selected.is_empty());
        let count = selected.len();
        let mut completed = 0;
        for case in selected {
            let id = case["id"].as_str().unwrap();
            let world = fixture_world(&case);
            let mut previous = None;
            for checked in [false, true] {
                let engine = if checked { "checked-h" } else { "reference" };
                if std::env::var("JOURNAL_KV_29_ENGINE").is_ok_and(|selected| selected != engine) {
                    continue;
                }
                eprintln!("JOURNAL_KV_29_STARTED {id} {engine}");
                let execution = if checked {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world.clone())
                } else {
                    hirsel_engine::run_observed_snapshot(&mut loaded.engine, &config, world.clone())
                }
                .unwrap();
                fs::write(
                    root.path().join(format!("{id}-{engine}.observation")),
                    format!("{execution:?}"),
                )
                .unwrap();
                if boundaries
                    && matches!(
                        execution.outcome,
                        Ok(hirsel_engine::RunOutcome::ToolingLimit(_))
                    )
                {
                    eprintln!(
                        "JOURNAL_KV_29_INCOMPLETE {id} {engine}: {:?}",
                        execution.outcome
                    );
                    completed += 1;
                    continue;
                }
                let obs = execution
                    .observation
                    .as_ref()
                    .unwrap_or_else(|| panic!("{id}/{engine}: {execution:?}"));
                assert_eq!(
                    obs.exit_code,
                    Some(case["exit"].as_i64().unwrap() as i32),
                    "{id}/{engine}: {execution:?}"
                );
                assert_eq!(
                    obs.output,
                    case["stdout"].as_str().unwrap().as_bytes(),
                    "{id}/{engine}"
                );
                assert!(
                    obs.failures.is_empty() && obs.blocked.is_none() && obs.signaled.is_none(),
                    "{id}/{engine}: {obs:?}"
                );
                assert_error_class(obs, case["error"].as_str());
                let expected: std::collections::BTreeMap<_, _> = case["expected_files"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), fixture_bytes(v)))
                    .collect();
                assert_eq!(
                    obs.final_state.as_ref().unwrap().trees[&0].1,
                    expected,
                    "{id}/{engine}"
                );
                let operations: Vec<&str> = obs
                    .effects
                    .iter()
                    .filter_map(|e| match &e.effect {
                        hirsel_interp::observation::Effect::CapabilityConsumed(event) => {
                            Some(event.operation.as_str())
                        }
                        _ => None,
                    })
                    .collect();
                if case["args"][0] == "apply" && case["exit"] == 0 && case["stdout"] != "" {
                    let mut append = None;
                    let mut sync = None;
                    let mut acks = 0;
                    for (n, op) in operations.iter().enumerate() {
                        match *op {
                            "file.append" => append = Some(n),
                            "file.sync" => sync = Some(n),
                            "out.print" => {
                                assert!(
                                    sync > append && append.is_some(),
                                    "{id}: ack before sync {operations:?}"
                                );
                                acks += 1;
                            }
                            _ => {}
                        }
                    }
                    assert_eq!(acks, case["stdout"].as_str().unwrap().lines().count());
                }
                if id == "oversized-length" {
                    assert_eq!(
                        operations.iter().filter(|&&op| op == "file.read").count(),
                        1
                    );
                }
                if let Some(previous) = previous {
                    assert_eq!(
                        *obs, previous,
                        "{id}: semantic order, ownership and effects"
                    );
                }
                previous = Some(obs.clone());
                completed += 1;
                eprintln!("JOURNAL_KV_29_PASS {id} {engine}");
            }
        }
        let engines = if std::env::var_os("JOURNAL_KV_29_ENGINE").is_some() {
            1
        } else {
            2
        };
        assert_eq!(completed, count * engines);
    });
}
#[test]
fn journal_kv_29_application_native_replay() {
    let root = Scratch::new();
    app_package(root.path());
    fs::write(
        root.path().join("cases.json"),
        include_str!("../support/journal_kv_29/cases.json"),
    )
    .unwrap();
    fs::write(
        root.path().join("verify.py"),
        include_str!("../support/journal_kv_29/verify.py"),
    )
    .unwrap();
    fs::write(
        root.path().join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let mut command = Command::new("python3");
    command.arg(root.path().join("verify.py"));
    command
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(root.path());
    if let Ok(filter) = std::env::var("JOURNAL_KV_29_CASE") {
        for id in filter.split(',') {
            command.arg("--case").arg(id);
        }
    }
    let done = command.output().unwrap();
    fs::write(root.path().join("native.stdout"), &done.stdout).unwrap();
    fs::write(root.path().join("native.stderr"), &done.stderr).unwrap();
    assert!(
        done.status.success(),
        "evidence {}: {}\n{}",
        root.path().display(),
        String::from_utf8_lossy(&done.stdout),
        String::from_utf8_lossy(&done.stderr)
    );
    let last = String::from_utf8(done.stdout).unwrap();
    let report: serde_json::Value = serde_json::from_str(last.lines().last().unwrap()).unwrap();
    assert!(report["completed"].as_u64().unwrap() > 0);
    assert_eq!(report["completed"], report["passed"]);
    eprintln!("JOURNAL_KV_29_NATIVE {report}");
}

#[test]
fn journal_kv_29_crash_cancellation_integration() {
    hirsel_engine::with_compiler_stack(|| {
        let root = Scratch::new();
        let selection = app_package(root.path());
        fs::write(
            root.path().join("integration.hir"),
            include_str!("../support/journal_kv_29/integration.hir"),
        )
        .unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&selection).unwrap()).unwrap();
        manifest["sources"]
            .as_array_mut()
            .unwrap()
            .push(json!({"path":"integration.hir","module":["checks"]}));
        fs::write(&selection, manifest.to_string()).unwrap();
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded.check_for_execution().unwrap();
        for name in [
            "crash_append_and_compaction",
            "atomic_checkpoints",
            "canceled_preparation",
            "reopen_before_sync",
        ] {
            if std::env::var("JOURNAL_KV_29_INTEGRATION").is_ok_and(|selected| selected != name) {
                continue;
            }
            let mut entry = loaded.entry.clone();
            entry.0.modules = vec!["checks".into()];
            entry.0.name = name.into();
            let config = hirsel_engine::RunConfiguration {
                entry,
                hash_seed: 0,
                inputs: vec![],
            };
            for checked in [false, true] {
                let mut world = hirsel_engine::SimulationConfig::default();
                world.grants.insert("Test".into());
                let execution = if checked {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world)
                } else {
                    hirsel_engine::run_observed_snapshot(&mut loaded.engine, &config, world)
                }
                .unwrap();
                fs::write(
                    root.path().join(format!("{name}-{checked}.observation")),
                    format!("{execution:?}"),
                )
                .unwrap();
                let obs = execution.observation.as_ref().unwrap();
                assert_eq!(obs.exit_code, Some(0), "{name}/{checked}: {execution:?}");
                assert!(obs.failures.is_empty(), "{name}/{checked}: {execution:?}");
                eprintln!("JOURNAL_KV_29_PASS integration {name} checked={checked}");
            }
        }
    });
}

// Audits a retained full-app campaign without rebuilding or reissuing its real
// providers. Explicit opt-in keeps raw tapes outside the permanent source tree.

#[test]
fn journal_kv_29_native_crash_helpers() {
    let root = Scratch::new();
    let selection = app_package(root.path());
    fs::write(
        root.path().join("integration.hir"),
        include_str!("../support/journal_kv_29/integration.hir"),
    )
    .unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&selection).unwrap()).unwrap();
    manifest["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"path":"integration.hir","module":["checks"]}));
    fs::write(&selection, manifest.to_string()).unwrap();
    fs::write(
        root.path().join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    for backend in ["llvm", "cranelift"] {
        let done = Command::new("python3")
            .arg(root.path().join("capped"))
            .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
            .arg(env!("CARGO_BIN_EXE_hirsel"))
            .args(["test", "--native", "--backend", backend])
            .arg(&selection)
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .env("HIRSEL_JOBS", "4")
            .output()
            .unwrap();
        fs::write(root.path().join(format!("{backend}.stdout")), &done.stdout).unwrap();
        fs::write(root.path().join(format!("{backend}.stderr")), &done.stderr).unwrap();
        assert!(done.status.success(), "{backend}: {done:?}");
        assert!(
            String::from_utf8_lossy(&done.stdout).contains("4 passed"),
            "{backend}: {done:?}"
        );
        eprintln!("JOURNAL_KV_29_PASS 4 native crash/cancellation helpers {backend}");
    }
}
