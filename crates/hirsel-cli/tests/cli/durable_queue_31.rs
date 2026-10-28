//! Full workqueue entry: explicit modules, Args, file/network/task projections.
use hirsel_engine::*;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn evidence() -> PathBuf {
    let base = std::env::var_os("DURABLE_QUEUE_31_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix("workqueue-")
        .tempdir_in(base)
        .unwrap()
        .keep()
}
fn package(root: &Path) -> PathBuf {
    for (name, source) in [
        (
            "main",
            include_str!("../../../../examples/workqueue/main.hir"),
        ),
        (
            "record",
            include_str!("../../../../examples/workqueue/record.hir"),
        ),
        (
            "log",
            include_str!("../../../../examples/workqueue/log.hir"),
        ),
        (
            "groups",
            include_str!("../../../../examples/workqueue/groups.hir"),
        ),
        (
            "dispatch",
            include_str!("../../../../examples/workqueue/dispatch.hir"),
        ),
        (
            "snapshot",
            include_str!("../../../../examples/workqueue/snapshot.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/workqueue/hirsel.json")).unwrap();
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
fn cases() -> Vec<Value> {
    let mut cases: Vec<Value> =
        serde_json::from_str(include_str!("../support/durable_queue_31/cases.json")).unwrap();
    cases.extend(
        serde_json::from_str::<Vec<Value>>(include_str!(
            "../support/durable_queue_31/boundaries.json"
        ))
        .unwrap(),
    );
    if let Ok(filter) = std::env::var("DURABLE_QUEUE_31_CASE") {
        cases.retain(|c| filter.split(',').any(|id| c["id"] == id));
    }
    assert!(!cases.is_empty(), "zero selected cases");
    cases
}
fn network(case: &Value) -> Value {
    let clients: Vec<_> = case["scripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|steps| json!({"address":"0.0.0.0","port":0,"tls":"plain","steps":steps}))
        .collect();
    json!({"provider":"net.script@2","rules":[{"address":"/fixture/workqueue.sock","service":"0","operation":"unix_listen"}],"peers":[],"listeners":[{"address":"/fixture/workqueue.sock","port":0,"clients":clients}]})
}
#[test]
fn durable_queue_31_reference_checked() {
    with_compiler_stack(|| {
        let root = evidence();
        let manifest = package(&root);
        let prepared = hirsel_driver::request_tir(&manifest, Verify::On, None)
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut completed = 0;
        for case in cases() {
            let id = case["id"].as_str().unwrap();
            let mut config = SimulationConfig {
                args: serde_json::from_value(case["args"].clone()).unwrap(),
                net: Some(serde_json::from_value(network(&case)).unwrap()),
                ..Default::default()
            };
            config.grants.insert("MonotonicClock".into());
            config.projections.insert("monotonic_clock".into());
            config.mounts = [("data".into(), hirsel_run_schema::Access::ReadWrite)].into();
            config.directories.insert("data".into());
            for (name, bytes) in case["files"].as_object().unwrap() {
                config.files.insert(name.clone(), fixture_bytes(bytes));
            }
            if let Some(script) = case.get("file_reads") {
                config.file_reads = serde_json::from_value(script.clone()).unwrap();
            }
            if let Some(faults) = case["fault"].as_object() {
                for (operation, fault) in faults {
                    config.faults.insert(
                        operation.clone(),
                        [hirsel_interp::library::simulation::Fault::Fault {
                            code: fault["code"].as_u64().unwrap() as u32,
                            partial: fault["partial"].as_u64().unwrap() as usize,
                        }]
                        .into(),
                    );
                }
            }
            let mut prior = None;
            for engine in ["reference", "checked-h"] {
                if std::env::var("DURABLE_QUEUE_31_ENGINE").is_ok_and(|s| s != engine) {
                    continue;
                }
                eprintln!("DURABLE_QUEUE_31_STARTED {id}/{engine}");
                let steps = case["steps"].as_u64().unwrap();
                let execution = hirsel_driver::testing::observe_tir(
                    &prepared,
                    &run,
                    config.clone(),
                    hirsel_interp::Limits {
                        steps: Some(steps),
                        ..Default::default()
                    },
                    engine == "checked-h",
                )
                .unwrap();
                fs::write(
                    root.join(format!("{id}-{engine}.receipt")),
                    format!("{execution:#?}"),
                )
                .unwrap();
                let observed = execution
                    .observation
                    .as_ref()
                    .unwrap_or_else(|| panic!("{id}/{engine}: {execution:?}"));
                assert!(
                    observed.failures.is_empty() && observed.blocked.is_none(),
                    "{id}/{engine}: {observed:?}"
                );
                assert_eq!(
                    observed.output,
                    case["stdout"].as_str().unwrap().as_bytes(),
                    "{id}/{engine}"
                );
                assert_eq!(
                    observed.exit_code,
                    case["exit"].as_i64().map(|n| n as i32),
                    "{id}/{engine}"
                );
                if case.get("file_reads").is_some() {
                    assert!(observed.effects.iter().any(|e| matches!(&e.effect, hirsel_interp::observation::Effect::CapabilityConsumed(event) if event.operation == "file.read.pending")), "{id}/{engine}: read never parked");
                }
                let state = observed.final_state.as_ref().unwrap();
                assert!(state.stderr.is_empty(), "{id}/{engine}: {state:?}");
                if let Some(expected) = case.get("log") {
                    assert_eq!(
                        state.trees[&0].1.get("data/queue.log"),
                        Some(&serde_json::from_value::<Vec<u8>>(expected.clone()).unwrap()),
                        "{id}/{engine}"
                    );
                }
                if let Some(prior) = prior {
                    assert_eq!(*observed, prior, "{id} complete observations");
                }
                prior = Some(observed.clone());
                completed += 1;
                eprintln!(
                    "DURABLE_QUEUE_31_PASS {id}/{engine} {:?}",
                    execution.step_usage
                );
            }
        }
        assert!(completed > 0);
    });
}
#[test]
fn durable_queue_31_host_ordered_and_replay() {
    let root = evidence();
    package(&root);
    fs::write(
        root.join("cases.json"),
        serde_json::to_vec(&cases()).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("verify.py"),
        include_str!("../support/durable_queue_31/verify.py"),
    )
    .unwrap();
    fs::write(
        root.join("host.py"),
        include_str!("../support/durable_queue_31/host.py"),
    )
    .unwrap();
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(root.join("verify.py"))
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(&root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &output.stdout).unwrap();
    fs::write(root.join("native.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "{}: {}\n{}",
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn durable_queue_31_fixture_native_live() {
    fixture_campaign(std::env::var_os("DURABLE_QUEUE_31_LIVE_ONLY").is_none())
}

fn fixture_campaign(replay_network: bool) {
    use hirsel_driver::native::*;
    with_compiler_stack(|| {
        let root = evidence();
        let manifest = package(&root);
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let snapshot = loaded
            .engine
            .export_native_input()
            .unwrap_or_else(|e| panic!("{e:?}"));
        let input = loaded
            .engine
            .import_native_input(snapshot)
            .unwrap_or_else(|e| panic!("{e:?}"));
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let ready = build(&input, &options).unwrap_or_else(|e| panic!("{e}"));
        let sites = Sites::new(input.snapshot());
        let matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::None,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let mut tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        tools.wrapper = vec![
            "/usr/bin/env".into(),
            "HIRSEL_MEMORY_LIMIT=2G".into(),
            "HIRSEL_JOBS=4".into(),
            "/usr/bin/python3".into(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tools/capped")
                .to_string_lossy()
                .into(),
            "--timeout".into(),
            "60".into(),
            "prlimit".into(),
            "--cpu=60:61".into(),
        ];
        let mut objects = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
            eprintln!("DURABLE_QUEUE_31_EMIT {backend:?}");
            objects.push((
                backend,
                emit_backend_objects(
                    &ready,
                    backend,
                    OptLevel::None,
                    Partition::Whole,
                    false,
                    None,
                )
                .unwrap(),
            ));
        }
        for case in cases() {
            let id = case["id"].as_str().unwrap();
            if case["native_fault_unavailable"] == true {
                eprintln!(
                    "DURABLE_QUEUE_31_INCOMPLETE {id}/native fault queue absent from public Fixture; original reference cells separate"
                );
                continue;
            }
            let fixture = Fixture {
                args: serde_json::from_value(case["args"].clone()).unwrap(),
                files: case["files"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(path, bytes)| (path.clone(), fixture_bytes(bytes)))
                    .collect(),
                mounts: [("data".into(), hirsel_run_schema::Access::ReadWrite)].into(),
                file_reads: case
                    .get("file_reads")
                    .map(|v| serde_json::from_value(v.clone()).unwrap())
                    .unwrap_or_default(),
                monotonic: Some(0),
                net: Some(serde_json::from_value(network(&case)).unwrap()),
                steps: case["steps"].as_u64(),
                ..Default::default()
            };
            let mut runnables = vec![];
            for (backend, objects) in &objects {
                let dir = root.join(format!("{id}-{backend:?}"));
                fs::create_dir(&dir).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &loaded.entry, objects, &tools, &dir, &fixture)
                        .unwrap();
                if let Ok(width) = std::env::var("DURABLE_QUEUE_31_SCRIPT_WIDTH") {
                    let mut run: Value =
                        serde_json::from_slice(&fs::read(dir.join("run.json")).unwrap()).unwrap();
                    run["placement"] = json!({"cores":width.parse::<u64>().unwrap(),"commit":"shared","reorder":"auto"});
                    fs::write(dir.join("run.json"), run.to_string()).unwrap();
                }
                // Native images/runners may be cache-owned. Retain these exact
                // consumed artifacts before action cleanup for offline witnesses.
                let destination = if let Runnable::Image(image) = &runnable {
                    fs::copy(&image.runner.path, dir.join("runner")).unwrap();
                    dir.join("program.image")
                } else {
                    dir.join("program")
                };
                fs::copy(runnable.artifact_path(), destination).unwrap();
                runnables.push((backend, runnable, dir));
            }
            for (backend, runnable, dir) in &runnables {
                eprintln!("DURABLE_QUEUE_31_STARTED {id}/{backend:?}");
                if std::env::var("DURABLE_QUEUE_31_SKIP_NATIVE").is_ok_and(|skip| {
                    skip.split(',')
                        .any(|cell| cell == format!("{id}/{backend:?}"))
                }) {
                    eprintln!(
                        "DURABLE_QUEUE_31_SKIPPED_CAPTURE {id}/{backend:?}; original receipt retained, zero new credit"
                    );
                    continue;
                }
                let resume = std::env::var_os("DURABLE_QUEUE_31_RESUME")
                    .map(PathBuf::from)
                    .map(|root| root.join(format!("{id}-{backend:?}")))
                    .filter(|dir| dir.join("observed.json").exists());
                let tape = resume.as_ref().unwrap_or(dir).join("original.tape");
                let observed = if let Some(original) = resume {
                    let value: Value =
                        serde_json::from_slice(&fs::read(original.join("observed.json")).unwrap())
                            .unwrap();
                    assert!(
                        value["failures"].as_array().unwrap().is_empty()
                            && value["budget"].is_null()
                            && value["signaled"].is_null()
                    );
                    eprintln!(
                        "DURABLE_QUEUE_31_REUSED_CAPTURE {id}/{backend:?}; actual archived observation, zero new live credit"
                    );
                    Observation {
                        exit_code: value["exit_code"].as_i64().map(|n| n as i32),
                        blocked: value["blocked"].as_bool().unwrap(),
                        budget: None,
                        signaled: None,
                        output: serde_json::from_value(value["output"].clone()).unwrap(),
                        failures: vec![],
                        roots: serde_json::from_value(value["roots"].clone()).unwrap(),
                    }
                } else {
                    let recorded = launch_fixture(
                        runnable,
                        &ready.target,
                        &tools,
                        dir,
                        true,
                        Tape::Live {
                            record: Some(&tape),
                            revocations: None,
                        },
                        &fixture,
                    )
                    .unwrap();
                    let value = observe(&recorded, &ready, &sites).unwrap();
                    fs::write(dir.join("receipt"), format!("{recorded:#?}\n{value:#?}")).unwrap();
                    assert!(
                        value.failures.is_empty()
                            && value.budget.is_none()
                            && value.signaled.is_none(),
                        "cannot archive a stopped observation as a successful capture"
                    );
                    fs::write(dir.join("observed.json"),json!({"exit_code":value.exit_code,"blocked":value.blocked,"budget":null,"signaled":null,"output":value.output,"failures":[],"roots":value.roots}).to_string()).unwrap();
                    eprintln!("DURABLE_QUEUE_31_PASS_LIVE {id}/{backend:?}");
                    value
                };
                assert_eq!(
                    observed.output,
                    case["stdout"].as_str().unwrap().as_bytes(),
                    "{id}/{backend:?}"
                );
                assert_eq!(observed.exit_code, case["exit"].as_i64().map(|n| n as i32));
                assert!(
                    observed.failures.is_empty() && !observed.blocked,
                    "{observed:?}"
                );
                let bytes = fs::read(&tape).unwrap();
                let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                if let Some(baseline) = std::env::var_os("DURABLE_QUEUE_31_WIDTH_BASE") {
                    let baseline =
                        PathBuf::from(baseline).join(format!("{id}-{backend:?}/original.tape"));
                    let mut baseline = hirsel_rt::host_launch::read_recording(&baseline).unwrap();
                    let mut current = record.clone();
                    for tape in [&mut baseline, &mut current] {
                        let binding = tape.binding.as_mut().unwrap();
                        binding.admitted_at = 0;
                        binding.placement = None;
                    }
                    assert_eq!(
                        current, baseline,
                        "{id}/{backend:?}: complete ordered events across requested widths"
                    );
                    eprintln!("DURABLE_QUEUE_31_WIDTH_EVENTS_PASS {id}/{backend:?}");
                }

                if case.get("file_reads").is_some() {
                    assert!(
                        record.events.iter().any(|e| matches!(
                            (&e.request, &e.completion),
                            (
                                hirsel_rt::io::Request::FileRead { .. },
                                hirsel_rt::io::Completion::Blocked { committed: false }
                            )
                        )),
                        "read never parked"
                    );
                }
                let unavailable = Fixture {
                    files: Default::default(),
                    file_reads: Default::default(),
                    net: None,
                    ..fixture.clone()
                };
                let uses_listen = record
                    .events
                    .iter()
                    .any(|e| matches!(e.request, hirsel_rt::io::Request::UnixListen { .. }));
                if uses_listen && !replay_network {
                    eprintln!(
                        "DURABLE_QUEUE_31_INCOMPLETE {id}/{backend:?} native replays=2 explicitly omitted by live-only partition"
                    );
                }
                for (target, executable, replay_dir) in &runnables {
                    if uses_listen && !replay_network {
                        continue;
                    }
                    let replayed = launch_fixture(
                        executable,
                        &ready.target,
                        &tools,
                        replay_dir,
                        true,
                        Tape::Replay(&tape),
                        &unavailable,
                    )
                    .unwrap();
                    assert_eq!(
                        observe(&replayed, &ready, &sites).unwrap(),
                        observed,
                        "{id}/{target:?} replay"
                    );
                    eprintln!("DURABLE_QUEUE_31_PASS_REPLAY {id}/{backend:?}->{target:?}");
                }
                for (name, replay) in [
                    (
                        "reference",
                        hirsel_driver::replay_observations
                            as fn(
                                &Path,
                                &Path,
                                Option<u64>,
                            )
                                -> Result<hirsel_interp::observation::Observable, ToolError>,
                    ),
                    ("checked-h", hirsel_driver::replay_observations_checked),
                ] {
                    let value = replay(&manifest, &tape, case["steps"].as_u64()).unwrap();
                    assert_eq!(Observation::of_reference(&value), observed, "{id}/{name}");
                    eprintln!("DURABLE_QUEUE_31_PASS_REPLAY {id}/{backend:?}->{name}");
                }
                assert_eq!(fs::read(&tape).unwrap(), bytes);
                eprintln!(
                    "DURABLE_QUEUE_31_CAPTURE_DONE {id}/{backend:?}; inspect individual replay dispositions"
                );
            }
        }
    });
}

fn fixture_bytes(value: &Value) -> Vec<u8> {
    if value.is_array() {
        return serde_json::from_value(value.clone()).unwrap();
    }
    let size = value["size"].as_u64().unwrap() as usize;
    assert!(size <= 524289);
    let mut bytes = vec![0; size];
    let prefix: Vec<u8> = serde_json::from_value(value["prefix"].clone()).unwrap();
    bytes[..prefix.len()].copy_from_slice(&prefix);
    bytes
}

#[test]
fn durable_queue_31_crash_cuts() {
    with_compiler_stack(|| {
        use hirsel_driver::native::*;
        let root = evidence();
        let manifest = package(&root);
        fs::write(
            root.join("crash.hir"),
            include_str!("../support/durable_queue_31/crash.hir"),
        )
        .unwrap();
        let mut value: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        value["sources"]
            .as_array_mut()
            .unwrap()
            .push(json!({"path":"crash.hir","module":["crash"]}));
        value["entry"] = json!({"module":["crash"],"name":"trial"});
        fs::write(&manifest, value.to_string()).unwrap();
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let plain = run_observed_snapshot_bounded(
            &mut loaded.engine,
            &run,
            SimulationConfig::default(),
            10_000_000,
        )
        .unwrap();
        let checked = run_observed_checked_bounded(
            &mut loaded.engine,
            &run,
            SimulationConfig::default(),
            10_000_000,
        )
        .unwrap();
        fs::write(root.join("crash-reference.receipt"), format!("{plain:#?}")).unwrap();
        fs::write(root.join("crash-checked.receipt"), format!("{checked:#?}")).unwrap();
        let observed = plain.observation.as_ref().unwrap();
        assert!(
            observed.failures.is_empty() && observed.blocked.is_none(),
            "{observed:?}"
        );
        assert_eq!(observed.exit_code, Some(0));
        assert_eq!(plain.observation, checked.observation);
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::None,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        let fixture = Fixture {
            steps: Some(10_000_000),
            ..Default::default()
        };
        for (configuration, outcome) in differential_in(
            &input,
            &loaded.entry,
            &Observation::of_reference(observed),
            &matrix,
            &tools,
            &root,
            &fixture,
        ) {
            assert_eq!(outcome, Outcome::Agree, "{configuration}");
        }
        eprintln!(
            "DURABLE_QUEUE_31_CRASH_PASS reference checked-h LLVM Cranelift (supplemental Test entry)"
        );
    });
}

#[test]
#[ignore = "consume already recorded workqueue tapes via DURABLE_QUEUE_31_TAPES; no live effects"]
fn durable_queue_31_existing_tapes_and_refusal_controls() {
    with_compiler_stack(|| {
        let root = PathBuf::from(std::env::var_os("DURABLE_QUEUE_31_TAPES").unwrap());
        let manifest = root.join("hirsel.json");
        let prepared = hirsel_driver::request_tir(&manifest, Verify::On, None).unwrap();
        let selected: Vec<Value> =
            serde_json::from_slice(&fs::read(root.join("tape-index.json")).unwrap()).unwrap();
        let controls = root.join("controls");
        fs::create_dir_all(&controls).unwrap();
        for case in &selected {
            if std::env::var_os("DURABLE_QUEUE_31_SKIP_EXISTING_REPLAYS").is_some() {
                continue;
            }
            let tape = root.join(case["tape"].as_str().unwrap());
            let bytes = fs::read(&tape).unwrap();
            let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
            for (engine, checked) in [("reference", false), ("checked-h", true)] {
                if std::env::var("DURABLE_QUEUE_31_REPLAY_ENGINE")
                    .is_ok_and(|selected| selected != engine)
                {
                    continue;
                }
                let observed = hirsel_driver::replay_prepared_observations(
                    &prepared,
                    &tape,
                    Some(30_000_000),
                    checked,
                )
                .unwrap();
                fs::write(
                    controls.join(format!("{}-{engine}.receipt", case["id"].as_str().unwrap())),
                    format!("{observed:#?}"),
                )
                .unwrap();
                assert_eq!(observed.output, case["stdout"].as_str().unwrap().as_bytes());
                assert_eq!(observed.exit_code, case["exit"].as_i64().map(|n| n as i32));
                assert!(observed.failures.is_empty() && observed.blocked.is_none());
            }
            assert_eq!(fs::read(tape).unwrap(), bytes);
            assert!(record.binding.is_some());
            eprintln!(
                "DURABLE_QUEUE_31_EXISTING_REPLAY_PASS {} selected adapters (see receipts)",
                case["id"]
            );
        }
        let original = root.join(selected[0]["tape"].as_str().unwrap());
        let record = hirsel_rt::host_launch::read_recording(&original).unwrap();
        for kind in ["input", "config", "extra", "missing"] {
            let mut invalid = record.clone();
            match kind {
                "input" => invalid.arguments.push("changed".into()),
                "config" => {
                    let mut config = invalid.bound_run().unwrap().record().clone();
                    config.io.as_mut().unwrap().stdout = None;
                    invalid.binding.as_mut().unwrap().canonical =
                        config.validated().unwrap().canonical().to_vec();
                }
                "extra" => invalid.events.push(record.events[0].clone()),
                "missing" => {
                    invalid.events.remove(0);
                }
                _ => unreachable!(),
            }
            let path = controls.join(format!("{kind}.tape"));
            fs::write(
                &path,
                hirsel_rt::launch::render_recording(&invalid).unwrap(),
            )
            .unwrap();
            let mut completed_controls = Vec::new();
            for (engine, checked) in [("reference", false), ("checked-h", true)] {
                if std::env::var("DURABLE_QUEUE_31_CONTROL_CELLS").is_ok_and(|cells| {
                    !cells
                        .split(',')
                        .any(|cell| cell == format!("{kind}/{engine}"))
                }) {
                    continue;
                }
                assert!(
                    hirsel_driver::replay_prepared_observations(
                        &prepared,
                        &path,
                        Some(30_000_000),
                        checked
                    )
                    .is_err(),
                    "{kind} accepted"
                );
                completed_controls.push(engine);
            }
            for backend in ["llvm", "cranelift"] {
                if std::env::var("DURABLE_QUEUE_31_CONTROL_CELLS").is_ok_and(|cells| {
                    !cells
                        .split(',')
                        .any(|cell| cell == format!("{kind}/{backend}"))
                }) {
                    continue;
                }
                let output = Command::new(root.join(backend))
                    .arg("--hirsel-replay")
                    .arg(&path)
                    .output()
                    .unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(3),
                    "{kind}/{backend}: {output:?}"
                );
                fs::write(
                    controls.join(format!("{kind}-{backend}.stdout")),
                    &output.stdout,
                )
                .unwrap();
                fs::write(
                    controls.join(format!("{kind}-{backend}.stderr")),
                    &output.stderr,
                )
                .unwrap();
                let expected = if kind == "extra" {
                    selected[0]["stdout"].as_str().unwrap().as_bytes()
                } else {
                    b""
                };
                assert_eq!(output.stdout, expected, "{kind}/{backend} output prefix");
                completed_controls.push(backend);
            }
            if !completed_controls.is_empty() {
                eprintln!("DURABLE_QUEUE_31_CONTROL_PASS {kind} {completed_controls:?}");
            }
        }
    });
}

fn audit_events(record: &hirsel_rt::launch::Recording, case: &Value) {
    use hirsel_rt::io::{Completion as C, Request as R};
    use std::collections::BTreeMap;
    let mut log = case["files"]
        .get("data/queue.log")
        .map(fixture_bytes)
        .unwrap_or_default();
    let mut appends = BTreeMap::new();
    let mut atomic = BTreeMap::<u64, Vec<u8>>::new();
    let mut unsynced = false;
    for event in &record.events {
        match (&event.request, &event.completion) {
            (R::OpenAppend { path, .. }, C::Handle(Ok(file))) if path == "queue.log" => {
                appends.insert(*file, ());
            }
            (R::CreateAtomic { path, .. }, C::Handle(Ok(file))) if path == "queue.log" => {
                atomic.insert(*file, vec![]);
            }
            (R::FileAppend { file, bytes, .. }, C::Unit(Ok(()))) if appends.contains_key(file) => {
                log.extend_from_slice(bytes);
                unsynced = true;
            }
            (R::WriteAll { file, bytes, .. }, C::Unit(Ok(()))) if atomic.contains_key(file) => {
                atomic.get_mut(file).unwrap().extend_from_slice(bytes);
                unsynced = true;
            }
            (R::Commit { file, .. }, C::Unit(Ok(()))) if atomic.contains_key(file) => {
                log = atomic.remove(file).unwrap();
            }
            (R::Sync { .. }, C::Unit(Ok(()))) => unsynced = false,
            (R::NetWriteAll { bytes, .. }, C::NetUnit(Ok(()))) if bytes.len() > 4 => {
                let reply: Value = serde_json::from_slice(&bytes[4..]).unwrap();
                if reply.get("enqueued").is_some()
                    || reply.get("id").is_some()
                    || reply["acked"] == 1
                    || reply.get("checkpointed").is_some()
                {
                    assert!(!unsynced, "success reply before a durable sync");
                }
            }
            _ => (),
        }
    }
    if let Some(expected) = case.get("log") {
        assert_eq!(log, fixture_bytes(expected), "independent final log bytes");
    }
}

#[test]
#[ignore = "audit immutable recorded workqueue effects; select with DURABLE_QUEUE_31_NATIVE_RECORDS"]
fn durable_queue_31_retained_event_audit() {
    let root = PathBuf::from(std::env::var_os("DURABLE_QUEUE_31_NATIVE_RECORDS").unwrap());
    let mut audited = 0;
    for case in cases() {
        for backend in ["Llvm", "Cranelift"] {
            let tape = root.join(format!(
                "{}-{backend}/original.tape",
                case["id"].as_str().unwrap()
            ));
            if !tape.exists() {
                continue;
            }
            let record = hirsel_rt::host_launch::read_recording(&tape).unwrap();
            audit_events(&record, &case);
            audited += 1;
        }
    }
    assert!(audited > 0);
    eprintln!(
        "DURABLE_QUEUE_31_EVENT_AUDIT_PASS {audited} original tapes; final bytes and sync-before-reply"
    );
}

#[test]
#[ignore = "full archival native record/replay matrix; select explicitly with retained evidence"]
fn durable_queue_31_fixture_native_record_replay() {
    fixture_campaign(true)
}
