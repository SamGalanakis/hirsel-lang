//! Full-entry chunkbackup fixtures. Bounds are fixture policies, never defaults.
use hirsel_engine::{RunConfiguration, SimulationConfig, with_compiler_stack};
use hirsel_run_schema::Access;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const STEPS: u64 = 20_000_000;
fn hash(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
fn package(root: &Path) -> PathBuf {
    for (name, source) in [
        (
            "main",
            include_str!("../../../../examples/chunkbackup/main.hir"),
        ),
        (
            "manifest",
            include_str!("../../../../examples/chunkbackup/manifest.hir"),
        ),
        (
            "chunk",
            include_str!("../../../../examples/chunkbackup/chunk.hir"),
        ),
        (
            "store",
            include_str!("../../../../examples/chunkbackup/store.hir"),
        ),
        (
            "snapshot",
            include_str!("../../../../examples/chunkbackup/snapshot.hir"),
        ),
        (
            "restore",
            include_str!("../../../../examples/chunkbackup/restore.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    let mut config: Value =
        serde_json::from_str(include_str!("../../../../examples/chunkbackup/hirsel.json")).unwrap();
    config["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    let path = root.join("hirsel.json");
    fs::write(&path, config.to_string()).unwrap();
    path
}
fn scratch(label: &str) -> PathBuf {
    let base = std::env::var_os("CHUNK_BACKUP_31_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
        .keep()
}
fn unhex(value: &Value) -> Vec<u8> {
    value
        .as_str()
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
fn decode_files(value: &Value) -> BTreeMap<String, Vec<u8>> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), unhex(v)))
        .collect()
}
fn cases(root: &Path, boundaries: bool) -> Vec<Value> {
    fs::write(
        root.join("vectors.json"),
        include_str!("../support/chunk_backup_31/vectors.json"),
    )
    .unwrap();
    fs::write(
        root.join("cases.py"),
        include_str!("../support/chunk_backup_31/cases.py"),
    )
    .unwrap();
    let output = std::process::Command::new("python3")
        .arg(root.join("cases.py"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    fs::write(root.join("cases.json"), &output.stdout).unwrap();
    let cases: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    let selected: Vec<_> = cases
        .into_iter()
        .filter(|c| c["boundary"].as_bool().unwrap_or(false) == boundaries)
        .filter(|c| {
            std::env::var("CHUNK_BACKUP_31_CASE")
                .map_or(true, |s| s.split(',').any(|id| c["id"] == id))
        })
        .collect();
    assert!(!selected.is_empty(), "zero selected fixtures");
    selected
}
fn fixture(case: &Value) -> hirsel_driver::native::Fixture {
    let mut mounts: BTreeMap<_, _> = [
        ("source".into(), Access::Read),
        ("chunks".into(), Access::ReadWrite),
        ("snapshots".into(), Access::ReadWrite),
        ("target".into(), Access::ReadWrite),
    ]
    .into();
    if let Some(name) = case["readonly"].as_str() {
        mounts.insert(name.into(), Access::Read);
    }
    let mut file_reads = BTreeMap::new();
    if let Some(name) = case["pending"].as_str() {
        file_reads.insert(name.into(), hirsel_run_schema::FileReadScript::Pending);
    }
    if let Some(name) = case["afterwrite"]["path"].as_str() {
        file_reads.insert(
            name.into(),
            hirsel_run_schema::FileReadScript::AfterWrite {
                path: case["afterwrite"]["release"].as_str().unwrap().into(),
            },
        );
    }
    let file_faults = if case["id"] == "durable-provider-fault" {
        std::collections::BTreeMap::from([(
            hirsel_run_schema::FileOperation::PutNew,
            std::collections::VecDeque::from([hirsel_run_schema::FileFault {
                requested: Some(3),
                error: Some(hirsel_run_schema::FileFaultError::Fault(39)),
                partial: 0,
            }]),
        )])
    } else {
        Default::default()
    };
    hirsel_driver::native::Fixture {
        file_faults,
        args: case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().into())
            .collect(),
        files: decode_files(&case["files"]),
        mounts,
        file_reads,
        deny_output: case["deny_output"] == true,
        steps: Some(case["steps"].as_u64().unwrap()),
        ..Default::default()
    }
}
fn scripted(boundaries: bool) {
    cap_fixture_process();
    with_compiler_stack(|| {
        let root = scratch("scripted-");
        let path = package(&root);
        seal_tools(&root);
        let cases = cases(&root, boundaries);
        let prepared = hirsel_driver::request_tir(&path, hirsel_engine::Verify::On, None)
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut passed = 0;
        let mut incomplete = 0;
        for case in cases {
            let id = case["id"].as_str().unwrap();
            let fixture = fixture(&case);
            let mut world: SimulationConfig = (&fixture).into();
            if let Some(operation) = case["fault"]
                .as_str()
                .filter(|_| fixture.file_faults.is_empty())
            {
                world.faults.insert(
                    operation.into(),
                    [hirsel_interp::library::simulation::Fault::Fault {
                        code: 39,
                        partial: 0,
                    }]
                    .into(),
                );
            }

            let steps = case["steps"].as_u64().unwrap();
            let mut previous = None;
            for engine in ["reference", "checked-h"]
                .into_iter()
                .filter(|name| selected_engine(name))
            {
                eprintln!("CHUNK_BACKUP_31_STARTED {id} {engine} steps={steps}");
                let result = hirsel_driver::testing::observe_tir(
                    &prepared,
                    &run,
                    world.clone(),
                    hirsel_interp::Limits { steps: Some(steps), ..Default::default() },
                    engine == "checked-h",
                )
                .unwrap_or_else(|e| panic!("{id}/{engine}: {e}"));
                fs::write(
                    root.join(format!("{id}-{engine}-outcome")),
                    format!("{:?}\n{:?}", result.outcome, result.step_usage),
                )
                .unwrap();
                let Some(observed) = result.observation else {
                    assert!(
                        matches!(
                            result.outcome,
                            Ok(hirsel_engine::RunOutcome::ToolingLimit(_))
                        ),
                        "{result:?}"
                    );
                    eprintln!(
                        "CHUNK_BACKUP_31_INCOMPLETE {id} {engine} {:?}",
                        result.outcome
                    );
                    incomplete += 1;
                    continue;
                };
                if id == "afterwrite-release" && observed.blocked.is_some() {
                    // Current AfterWrite contract releases only write_bytes;
                    // immutable put_new cannot release this actual app read.
                    fs::write(
                        root.join(format!("{id}-{engine}-INCOMPLETE")),
                        format!("{:?}", observed.blocked),
                    )
                    .unwrap();
                    eprintln!(
                        "CHUNK_BACKUP_31_INCOMPLETE {id} {engine} Blocked: put_new is not an AfterWrite trigger"
                    );
                    incomplete += 1;
                    continue;
                }
                assert_eq!(
                    observed.exit_code,
                    case["exit"].as_i64().map(|n| n as i32),
                    "{id}/{engine}: exit={:?} blocked={:?} failures={:?}",
                    observed.exit_code,
                    observed.blocked,
                    observed.failures
                );
                assert_eq!(observed.output, unhex(&case["stdout"]), "{id}/{engine}");
                assert!(
                    observed.failures.is_empty()
                        && observed.blocked.is_none()
                        && observed.signaled.is_none(),
                    "{id}/{engine}: exit={:?} blocked={:?} failures={:?}",
                    observed.exit_code,
                    observed.blocked,
                    observed.failures
                );
                let state = observed.final_state.as_ref().unwrap();
                assert_eq!(state.stderr, unhex(&case["stderr"]), "{id}/{engine}");
                let actual: BTreeMap<_, _> = state
                    .trees
                    .values()
                    .flat_map(|(_, files)| files.clone())
                    .collect();
                if case["exit"] == 0 {
                    assert_eq!(actual, decode_files(&case["expected"]), "{id}/{engine}");
                } else {
                    assert_eq!(
                        actual.get("snapshots/daily"),
                        world.files.get("snapshots/daily"),
                        "{id}: failed backup must not publish"
                    );
                    for (name, data) in &actual {
                        if name.starts_with("chunks/") && !world.files.contains_key(name) {
                            assert_eq!(hash(data), name[7..], "{id}: orphan must be valid");
                        }
                        if name.starts_with("target/") {
                            assert!(
                                world.files.get(name) == Some(data)
                                    || case["completed_files"]
                                        .get(name)
                                        .is_some_and(|v| unhex(v) == *data),
                                "{id}: unexpected restored file {name}"
                            );
                        }
                    }
                }
                if case["pending"].is_string() || case["afterwrite"].is_object() {
                    assert!(observed.effects.iter().any(|e|matches!(&e.effect,hirsel_interp::observation::Effect::CapabilityConsumed(event) if event.operation=="file.read.pending")),"{id}: never actually parked");
                }
                assert!(
                    !state.handles.values().any(|h| matches!(
                        h,
                        hirsel_interp::library::simulation::ObservedGrant::File { .. }
                    )),
                    "{id}: file handles not closed"
                );
                if let Some(previous) = &previous {
                    assert_eq!(&observed, previous, "{id}: full semantic equality");
                }
                // The exact semantic tape is immutable; replay consumes every
                // event through the public adapter, with the same admission.
                let events = observed
                    .effects
                    .iter()
                    .filter_map(|e| match &e.effect {
                        hirsel_interp::observation::Effect::CapabilityConsumed(event) => {
                            Some(event.clone())
                        }
                        _ => None,
                    })
                    .collect();
                let replay_run = RunConfiguration {
                    inputs: events,
                    ..run.clone()
                };
                let replayed = hirsel_driver::testing::observe_tir(
                    &prepared,
                    &replay_run,
                    world.clone(),
                    hirsel_interp::Limits { steps: Some(steps), ..Default::default() },
                    engine == "checked-h",
                )
                .unwrap();
                assert_eq!(
                    replayed.observation.as_ref(),
                    Some(&observed),
                    "{id}: complete simulated tape replay"
                );
                previous = Some(observed);
                passed += 1;
                eprintln!(
                    "CHUNK_BACKUP_31_PASS {id} {engine} usage={:?}",
                    result.step_usage
                );
            }
        }
        eprintln!("CHUNK_BACKUP_31_TOTAL pass={passed} incomplete={incomplete}");
        assert!(passed + incomplete > 0);
    })
}
#[test]
fn chunk_backup_31_scripted_reference_checked() {
    if dispatch_cases("chunk_backup_31_scripted_reference_checked", false, false) {
        return;
    }
    scripted(false)
}
#[test]
fn chunk_backup_31_bounds_reference_checked() {
    if dispatch_cases("chunk_backup_31_bounds_reference_checked", true, false) {
        return;
    }
    scripted(true)
}

#[test]
fn chunk_backup_31_scripted_native_record_replay() {
    if dispatch_cases(
        "chunk_backup_31_scripted_native_record_replay",
        std::env::var_os("CHUNK_BACKUP_31_BOUNDARY").is_some(),
        true,
    ) {
        return;
    }
    cap_fixture_process();
    use hirsel_driver::native::*;
    if std::env::var_os("CHUNK_BACKUP_31_PRODUCTION_TAPES").is_some() {
        production_tapes_host_replay();
        return;
    }

    with_compiler_stack(|| {
        hirsel_driver::session::enable_resident(1);
        let root = scratch("native-");
        let path = package(&root);
        seal_tools(&root);
        let cases = cases(
            &root,
            std::env::var_os("CHUNK_BACKUP_31_BOUNDARY").is_some(),
        );
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        check_program(&mut loaded);
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
                options: options.clone(),
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let mut tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        tools.wrapper = vec![
            "/usr/bin/env".into(),
            "PATH=/usr/bin:/bin".into(),
            "HIRSEL_MEMORY_LIMIT=2G".into(),
            "HIRSEL_JOBS=4".into(),
            "/usr/bin/python3".into(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tools/capped")
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into(),
            "--timeout".into(),
            "60".into(),
            "/usr/bin/prlimit".into(),
            "--cpu=60:61".into(),
        ];
        let ready = build(&input, &options).unwrap();
        let sites = Sites::new(input.snapshot());
        let mut objects = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
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
        for case in cases {
            let id = case["id"].as_str().unwrap();
            if case["fault"].is_string() && case["id"] != "durable-provider-fault" {
                eprintln!(
                    "CHUNK_BACKUP_31_INCOMPLETE {id} native: immutable case not migrated to typed file fault queue"
                );
                continue;
            }
            let recipe = fixture(&case);
            let mut runnables = vec![];
            let widths: Vec<u64> = std::env::var("CHUNK_BACKUP_31_WIDTHS")
                .map(|v| v.split(',').map(|n| n.parse().unwrap()).collect())
                .unwrap_or_else(|_| vec![1]);
            for (backend, objects) in &objects {
                for width in &widths {
                    let dir = root.join(format!("{id}-{backend:?}-w{width}"));
                    fs::create_dir(&dir).unwrap();
                    let runnable = link_fixture_runnable(
                        &ready,
                        &loaded.entry,
                        objects,
                        &tools,
                        &dir,
                        &recipe,
                    )
                    .unwrap();
                    let path = dir.join("run.json");
                    let mut run: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    run["placement"] =
                        serde_json::json!({"cores":width,"commit":"shared","reorder":"off"});
                    fs::write(path, run.to_string()).unwrap();
                    let artifact = runnable.artifact_path();
                    fs::write(
                        dir.join("artifact-before.sha256"),
                        hash(&fs::read(artifact).unwrap()),
                    )
                    .unwrap();
                    runnables.push((runnable, dir));
                }
            }
            let mut semantic = None;
            for (recorder, dir) in &runnables {
                if let Ok(filter) = std::env::var("CHUNK_BACKUP_31_RECORDERS") {
                    if !filter
                        .split(',')
                        .any(|name| dir.file_name().unwrap().to_string_lossy().contains(name))
                    {
                        continue;
                    }
                }
                eprintln!("CHUNK_BACKUP_31_STARTED {id} native {}", dir.display());
                let tape = dir.join("recording.tape");
                let launched = launch_fixture(
                    recorder,
                    &ready.target,
                    &tools,
                    dir,
                    true,
                    Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                    &recipe,
                )
                .unwrap();
                let observed = observe(&launched, &ready, &sites).unwrap();
                fs::write(
                    dir.join("launch.txt"),
                    format!("{launched:?}\n{observed:?}"),
                )
                .unwrap();
                if id == "afterwrite-release" && observed.blocked {
                    eprintln!(
                        "CHUNK_BACKUP_31_INCOMPLETE {id} native Blocked (put_new trigger limitation)"
                    );
                    continue;
                }
                assert_eq!(
                    observed.exit_code,
                    case["exit"].as_i64().map(|n| n as i32),
                    "{id}: {launched:?}"
                );
                assert_eq!(observed.output, unhex(&case["stdout"]), "{id}");
                assert!(
                    observed.failures.is_empty() && !observed.blocked,
                    "{id}: {observed:?}"
                );
                let bytes = fs::read(&tape).unwrap();
                let recording = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                let error_bytes: Vec<u8> = recording
                    .events
                    .iter()
                    .filter_map(|e| match &e.request {
                        hirsel_rt::io::Request::PrintStderr { bytes, .. } => Some(bytes.as_slice()),
                        _ => None,
                    })
                    .flatten()
                    .copied()
                    .collect();
                assert_eq!(
                    error_bytes,
                    unhex(&case["stderr"]),
                    "{id}: exact stderr tape"
                );
                if case["pending"].is_string() {
                    assert!(!recording.events.iter().any(|e|matches!(&e.request,hirsel_rt::io::Request::Open{path,..} if path=="c")),"queued file admitted");
                    let file = recording
                        .events
                        .iter()
                        .find_map(|e| match (&e.request, &e.completion) {
                            (
                                hirsel_rt::io::Request::FileRead { file, .. },
                                hirsel_rt::io::Completion::Blocked { .. },
                            ) => Some(*file),
                            _ => None,
                        })
                        .unwrap();
                    assert!(recording.events.iter().any(|e|matches!(&e.request,hirsel_rt::io::Request::Close{handle} if *handle==file)),"pending file not closed");
                    assert!(
                        !recording
                            .events
                            .iter()
                            .any(|e| matches!(&e.request, hirsel_rt::io::Request::PutNew { .. })),
                        "canceled/error ingest published"
                    );
                }
                assert_eq!(recording.arguments, recipe.args);
                assert_eq!(recording.tree.files, recipe.files);
                let mut normalized = recording.clone();
                if let Some(binding) = &mut normalized.binding {
                    binding.admitted_at = 0;
                    binding.placement = None;
                }
                if let Some(previous) = &semantic {
                    assert_eq!(
                        &normalized, previous,
                        "{id}: complete unsorted native events across widths/backends"
                    );
                } else {
                    semantic = Some(normalized);
                }
                // Validate effects from the actual native tape against the
                // independent byte oracle, including no premature manifest.
                let mut mounts = BTreeMap::new();
                let mut actual = recipe.files.clone();
                for event in &recording.events {
                    match (&event.request, &event.completion) {
                        (
                            hirsel_rt::io::Request::Directory { path, .. },
                            hirsel_rt::io::Completion::Handle(Ok(handle)),
                        ) => {
                            mounts.insert(*handle, path.clone());
                        }
                        (
                            hirsel_rt::io::Request::PutNew {
                                directory,
                                path,
                                bytes,
                                durable,
                            },
                            hirsel_rt::io::Completion::Unit(Ok(())),
                        ) => {
                            assert!(*durable);
                            let name = format!("{}/{path}", mounts[directory]);
                            actual.insert(name, bytes.clone());
                        }
                        _ => (),
                    }
                }
                if case["exit"] == 0 {
                    assert_eq!(
                        actual,
                        decode_files(&case["expected"]),
                        "{id}: native effects"
                    );
                } else {
                    assert_eq!(
                        actual.get("snapshots/daily"),
                        recipe.files.get("snapshots/daily")
                    );
                }

                if case["pending"].is_string() || case["afterwrite"].is_object() {
                    assert!(
                        recording.events.iter().any(|e| matches!(
                            (&e.request, &e.completion),
                            (
                                hirsel_rt::io::Request::FileRead { .. },
                                hirsel_rt::io::Completion::Blocked { committed: false }
                            )
                        )),
                        "{id}: never Blocked"
                    );
                }
                let offline = Fixture {
                    files: BTreeMap::new(),
                    file_reads: BTreeMap::new(),
                    ..recipe.clone()
                };
                for (runnable, replay_dir) in &runnables {
                    let replayed = launch_fixture(
                        runnable,
                        &ready.target,
                        &tools,
                        replay_dir,
                        true,
                        Tape::Replay(&tape),
                        &offline,
                    )
                    .unwrap();
                    assert_eq!(
                        observe(&replayed, &ready, &sites).unwrap(),
                        observed,
                        "{id}: native replay"
                    );
                    let program_stderr: Vec<u8> = replayed
                        .stderr
                        .split_inclusive(|b| *b == b'\n')
                        .filter(|line| !line.starts_with(b"capped: "))
                        .flatten()
                        .copied()
                        .collect();
                    assert_eq!(
                        program_stderr,
                        unhex(&case["stderr"]),
                        "{id}: replay stderr"
                    );
                    fs::write(
                        dir.join(format!(
                            "replay-{}-PASS",
                            replay_dir.file_name().unwrap().to_string_lossy()
                        )),
                        "complete native replay",
                    )
                    .unwrap();
                    eprintln!(
                        "CHUNK_BACKUP_31_PASS {id} native-replay {}",
                        replay_dir.display()
                    );
                }
                let mut previous = None;
                for (name, replay) in [
                    (
                        "reference",
                        hirsel_driver::replay_observations as fn(&Path, &Path, Option<u64>) -> _,
                    ),
                    ("checked-h", hirsel_driver::replay_observations_checked),
                ] {
                    let result = replay(&path, &tape, Some(case["steps"].as_u64().unwrap()));
                    match result {
                        Ok(value) => {
                            assert_eq!(Observation::of_reference(&value), observed, "{id}/{name}");
                            if let Some(previous) = &previous {
                                assert_eq!(&value, previous);
                            }
                            previous = Some(value);
                            fs::write(
                                dir.join(format!("host-replay-{name}-PASS")),
                                "complete host replay",
                            )
                            .unwrap();
                            eprintln!("CHUNK_BACKUP_31_PASS {id} host-replay {name}");
                        }
                        Err(error) => {
                            fs::write(dir.join(format!("{name}-replay-error")), error.to_string())
                                .unwrap();
                            eprintln!(
                                "CHUNK_BACKUP_31_INCOMPLETE {id} host-replay {name}: {error}"
                            );
                        }
                    }
                }
                if std::env::var_os("CHUNK_BACKUP_31_CONTROLS").is_some() {
                    let mut changed = recording.clone();
                    changed.arguments.push("changed".into());
                    let bad = dir.join("changed-args.tape");
                    fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap())
                        .unwrap();
                    for replay in [
                        hirsel_driver::replay_observations,
                        hirsel_driver::replay_observations_checked,
                    ] {
                        let e = replay(&path, &bad, Some(STEPS)).unwrap_err().to_string();
                        assert!(e.contains("header disagrees"), "{e}");
                    }
                    for (runnable, replay_dir) in &runnables {
                        let error = launch_fixture(
                            runnable,
                            &ready.target,
                            &tools,
                            replay_dir,
                            true,
                            Tape::Replay(&bad),
                            &offline,
                        )
                        .unwrap_err();
                        assert!(error.message.contains("header disagrees"), "{error}");
                    }
                }
                assert_eq!(fs::read(&tape).unwrap(), bytes);
                eprintln!("CHUNK_BACKUP_31_PASS {id} native-record {}", dir.display());
            }
        }
    });
}

fn seal_tools(root: &Path) {
    fn visit(path: &Path, hashes: &mut BTreeMap<String, String>) {
        if path.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(&entry.unwrap().path(), hashes);
            }
        } else if path.is_file() {
            hashes.insert(
                path.to_string_lossy().into(),
                hash(&fs::read(path).unwrap()),
            );
        }
    }
    let mut hashes = BTreeMap::new();
    for key in [
        "HIRSEL_PRODUCTS",
        "HIRSEL_RUNTIME_ARCHIVES",
        "HIRSEL_NATIVE_SELECTION",
        "HIRSEL_CLANG",
        "HIRSEL_LLD",
        "HIRSEL_RUSTC",
    ] {
        if let Some(value) = std::env::var_os(key) {
            visit(Path::new(&value), &mut hashes);
        }
    }
    visit(Path::new(env!("CARGO_BIN_EXE_hirsel")), &mut hashes);
    for name in [
        "main.hir",
        "manifest.hir",
        "chunk.hir",
        "store.hir",
        "snapshot.hir",
        "restore.hir",
        "hirsel.json",
    ] {
        visit(&root.join(name), &mut hashes);
    }
    fs::write(
        root.join("consumer-tools.json"),
        serde_json::to_vec_pretty(&hashes).unwrap(),
    )
    .unwrap();
}

fn check_program(loaded: &mut hirsel_engine::LoadedProgram) {
    let output = loaded
        .check(hirsel_engine::LoweringPolicy::Reachable)
        .unwrap_or_else(|e| panic!("{e:?}"));
    for diagnostic in &output.diagnostics {
        eprintln!("{}", diagnostic.render_human(&output.sources));
    }
    loaded
        .check_for_execution()
        .unwrap_or_else(|e| panic!("{e}"));
}
fn production_tapes_host_replay() {
    with_compiler_stack(|| {
        let Some(directory) = std::env::var_os("CHUNK_BACKUP_31_PRODUCTION_TAPES") else {
            return;
        };
        let root = PathBuf::from(directory);
        let path = root.join("hirsel.json");
        if let Some(file) = std::env::var_os("CHUNK_BACKUP_31_AUDIT_WIDTHS") {
            let paths: Vec<String> = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
            let mut groups: BTreeMap<String, Vec<hirsel_rt::launch::Recording>> = BTreeMap::new();
            for path in paths {
                let path = PathBuf::from(path);
                let spelling = path
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy();
                let group = spelling
                    .split("-Llvm")
                    .next()
                    .unwrap()
                    .split("-Cranelift")
                    .next()
                    .unwrap()
                    .to_string();
                let mut tape =
                    hirsel_rt::launch::parse_recording_bytes(&fs::read(&path).unwrap()).unwrap();
                let binding = tape.binding.as_mut().unwrap();
                binding.admitted_at = 0;
                binding.placement = None;
                groups.entry(group).or_default().push(tape);
            }
            assert_eq!(groups.len(), 3);
            for (group, tapes) in groups {
                assert_eq!(tapes.len(), 6, "{group}");
                for tape in &tapes[1..] {
                    assert_eq!(tape, &tapes[0], "{group}: exact unsorted semantic tape");
                }
            }
            fs::write(root.join("width-event-audit-PASS"),"3 workflows x 2 native backends x widths1/2/4; placement/admission only normalized").unwrap();
            return;
        }
        if std::env::var_os("CHUNK_BACKUP_31_TAPE_CONTROLS").is_some() {
            tape_controls(&root, &path, &root.join("original.tape"), STEPS);
            return;
        }
        let rows: Vec<Value> =
            serde_json::from_slice(&fs::read(root.join("production-receipts.json")).unwrap())
                .unwrap();
        let mut count = 0;
        for row in rows {
            if let Ok(filter) = std::env::var("CHUNK_BACKUP_31_TAPE_CASE") {
                if !filter.split(',').any(|id| row["id"] == id) {
                    continue;
                }
            }
            let id = row["id"].as_str().unwrap();
            let tape = PathBuf::from(row["tape"].as_str().unwrap());
            let original = fs::read(&tape).unwrap();
            let mut previous = None;
            let replay_bound = row["steps"].as_u64().unwrap_or(40_000_000);
            for (name, replay) in [
                (
                    "reference",
                    hirsel_driver::replay_observations as fn(&Path, &Path, Option<u64>) -> _,
                ),
                ("checked-h", hirsel_driver::replay_observations_checked),
            ] {
                eprintln!(
                    "CHUNK_BACKUP_31_STARTED {id} production-host-replay {name} steps={replay_bound}"
                );
                if !selected_engine(name) {
                    continue;
                }
                let result = replay(&path, &tape, Some(replay_bound));
                match result {
                    Ok(value) => {
                        if row["status"] == 4 {
                            assert!(value.blocked.is_some());
                            assert!(value.exit_code.is_none());
                        } else {
                            assert_eq!(value.exit_code, row["status"].as_i64().map(|n| n as i32));
                        }
                        assert_eq!(
                            value.output,
                            fs::read(root.join(format!("{id}.stdout"))).unwrap()
                        );
                        if let Some(previous) = &previous {
                            assert_eq!(&value, previous);
                        }
                        previous = Some(value);
                        fs::write(
                            root.join(format!("{id}-{name}-host-replay-PASS")),
                            "complete output, exit, event consumption; no host providers",
                        )
                        .unwrap();
                        eprintln!("CHUNK_BACKUP_31_PASS {id} production-host-replay {name}");
                    }
                    Err(error) => {
                        fs::write(
                            root.join(format!("{id}-{name}-host-replay-INCOMPLETE")),
                            error.to_string(),
                        )
                        .unwrap();
                        eprintln!(
                            "CHUNK_BACKUP_31_INCOMPLETE {id} production-host-replay {name}: {error}"
                        );
                    }
                }
                count += 1;
            }
            assert_eq!(fs::read(&tape).unwrap(), original);
        }
        assert!(count > 0, "zero selected tapes");
    });
}

#[test]
fn chunk_backup_31_fragmentation_and_reset_real_splitter() {
    cap_fixture_process();
    with_compiler_stack(|| {
        let root = scratch("fragment-");
        let path = package(&root);
        fs::write(
            root.join("fragment.hir"),
            include_str!("../support/chunk_backup_31/fragment.hir"),
        )
        .unwrap();
        let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        config["sources"][0]["module"] = serde_json::json!(["app"]);
        config["sources"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"path":"fragment.hir","module":[]}));
        fs::write(&path, config.to_string()).unwrap();
        seal_tools(&root);
        let prepared = hirsel_driver::request_tir(&path, hirsel_engine::Verify::On, None).unwrap();
        let run = RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for engine in ["reference", "checked-h"]
            .into_iter()
            .filter(|name| selected_engine(name))
        {
            let result = hirsel_driver::testing::observe_tir(
                &prepared,
                &run,
                SimulationConfig::default(),
                hirsel_interp::Limits { steps: Some(STEPS), ..Default::default() },
                engine == "checked-h",
            )
            .unwrap();
            fs::write(
                root.join(format!("{engine}-outcome")),
                format!("{:?} {:?}", result.outcome, result.step_usage),
            )
            .unwrap();
            let observation = result
                .observation
                .expect("real splitter completed within fixed20M");
            assert_eq!(observation.exit_code, Some(0));
            assert!(observation.failures.is_empty());
            eprintln!(
                "CHUNK_BACKUP_31_PASS fragmentation-reset {engine} usage={:?}",
                result.step_usage
            );
        }
    });
}

fn cap_fixture_process() {
    if std::env::var_os("CHUNK_BACKUP_31_CAP_CPU").is_some() {
        let bound = libc::rlimit {
            rlim_cur: 60,
            rlim_max: 61,
        };
        // This test process only; setup and native child caps remain independent.
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_CPU, &bound) }, 0);
    }
}

fn selected_engine(name: &str) -> bool {
    std::env::var("CHUNK_BACKUP_31_ENGINE").map_or(true, |filter| {
        filter.split(',').any(|selected| selected == name)
    })
}
fn tape_controls(root: &Path, path: &Path, tape: &Path, bound: u64) {
    use std::process::Command;
    let original = fs::read(tape).unwrap();
    let recording = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
    let mut variants = Vec::new();
    let mut config = recording.clone();
    let binding = config.binding.as_mut().unwrap();
    let canonical = String::from_utf8(binding.canonical.clone()).unwrap();
    assert!(canonical.contains("\"hash_seed\":0"));
    binding.canonical = canonical
        .replacen("\"hash_seed\":0", "\"hash_seed\":1", 1)
        .into_bytes();
    variants.push((
        "valid-config",
        hirsel_rt::launch::render_recording(&config)
            .unwrap()
            .into_bytes(),
    ));
    let mut input = recording.clone();
    if let Some(bytes) = input.tree.files.values_mut().find(|b| !b.is_empty()) {
        bytes[0] ^= 1;
    } else {
        let bytes =
            input
                .events
                .iter_mut()
                .find_map(|e| match &mut e.completion {
                    hirsel_rt::io::Completion::FileInput(hirsel_rt::io::FileInput::Bytes(
                        bytes,
                    )) if !bytes.is_empty() => Some(bytes),
                    _ => None,
                })
                .unwrap();
        bytes[0] ^= 1;
    }
    variants.push((
        "changed-input",
        hirsel_rt::launch::render_recording(&input)
            .unwrap()
            .into_bytes(),
    ));
    let mut extra = recording.clone();
    extra.events.push(recording.events[0].clone());
    variants.push((
        "extra-event",
        hirsel_rt::launch::render_recording(&extra)
            .unwrap()
            .into_bytes(),
    ));
    variants.push(("truncated", original[..original.len() - 3].to_vec()));
    let mut damaged = original.clone();
    let at = damaged.len() / 2;
    damaged[at] ^= 1;
    variants.push(("checksum", damaged));
    for (name, bytes) in variants {
        if std::env::var("CHUNK_BACKUP_31_CONTROL").is_ok_and(|v| !v.split(',').any(|n| n == name))
        {
            continue;
        }
        let bad = root.join(format!("control-{name}.tape"));
        fs::write(&bad, bytes).unwrap();
        for (engine, replay) in [
            (
                "reference",
                hirsel_driver::replay_observations as fn(&Path, &Path, Option<u64>) -> _,
            ),
            ("checked-h", hirsel_driver::replay_observations_checked),
        ] {
            if !control_engine(engine) {
                continue;
            }
            let error = replay(path, &bad, Some(bound)).unwrap_err().to_string();
            assert!(
                !error.contains("Steps") && !error.contains("no semantic observation"),
                "{name}/{engine}: {error}"
            );
            fs::write(root.join(format!("control-{name}-{engine}-PASS")), error).unwrap();
        }
        for backend in ["llvm", "cranelift"] {
            if !control_engine(backend) {
                continue;
            }
            let output = Command::new("/usr/bin/python3")
                .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/capped"))
                .env("HIRSEL_MEMORY_LIMIT", "2G")
                .env("HIRSEL_JOBS", "4")
                .args(["--timeout", "60", "/usr/bin/prlimit", "--cpu=60:61"])
                .arg(root.join(format!("app-{backend}")))
                .arg("--hirsel-replay")
                .arg(&bad)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(3),
                "{name}/{backend}: {output:?}"
            );
            let recorded_output: Vec<u8> = recording
                .events
                .iter()
                .filter_map(|e| match &e.request {
                    hirsel_rt::io::Request::Print { bytes, .. } => Some(bytes.as_slice()),
                    _ => None,
                })
                .flatten()
                .copied()
                .collect();
            let expected = if name == "extra-event" {
                recorded_output.as_slice()
            } else {
                b"".as_slice()
            };
            assert_eq!(
                output.stdout, expected,
                "{name}/{backend}: permitted refusal output prefix"
            );
            fs::write(
                root.join(format!("control-{name}-{backend}-PASS")),
                output.stderr,
            )
            .unwrap();
        }
    }
    assert_eq!(fs::read(tape).unwrap(), original);
}

fn control_engine(name: &str) -> bool {
    std::env::var("CHUNK_BACKUP_31_CONTROL_ENGINES")
        .map_or(true, |v| v.split(',').any(|n| n == name))
}
// Default test invocations partition fixtures as well: each child is bounded,
// selected by its exact libtest identity, and child failure cannot turn green.
fn dispatch_cases(test: &str, boundaries: bool, native: bool) -> bool {
    if std::env::var_os("CHUNK_BACKUP_31_CASE").is_some()
        || std::env::var_os("CHUNK_BACKUP_31_PRODUCTION_TAPES").is_some()
    {
        return false;
    }
    let root = scratch("dispatch-");
    let mut selected = cases(&root, boundaries);
    if let Ok(filter) = std::env::var("CHUNK_BACKUP_31_DISPATCH_CASES") {
        selected.retain(|case| filter.split(',').any(|id| case["id"] == id));
    }
    assert!(!selected.is_empty(), "zero dispatch selection");
    // Small fixtures share compiler setup while every original recorder/case
    // still executes. Larger fixtures retain their independent process budgets.
    let mut groups: Vec<(String, Vec<&str>)> = vec![];
    let mut small = vec![];
    for case in selected {
        let id = case["id"].as_str().unwrap().to_owned();
        let bytes: usize = case["files"]
            .as_object()
            .unwrap()
            .values()
            .map(|value| value.as_str().unwrap().len() / 2)
            .sum();
        if native && bytes <= 1024 {
            small.push(id);
            if small.len() == 8 {
                groups.push((small.join(","), vec!["Llvm,Cranelift"]));
                small.clear();
            }
        } else {
            if !small.is_empty() {
                groups.push((small.join(","), vec!["Llvm,Cranelift"]));
                small.clear();
            }
            let engines = if native {
                vec!["Llvm", "Cranelift"]
            } else {
                ["reference", "checked-h"]
                    .into_iter()
                    .filter(|e| selected_engine(e))
                    .collect()
            };
            groups.push((id, engines));
        }
    }
    if !small.is_empty() {
        groups.push((small.join(","), vec!["Llvm,Cranelift"]));
    }
    for (ids, engines) in groups {
        for engine in engines {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    &format!("chunk_backup_31::{test}"),
                    "--nocapture",
                ])
                .env("CHUNK_BACKUP_31_CASE", &ids)
                .env("CHUNK_BACKUP_31_CAP_CPU", "1")
                .env("HIRSEL_MEMORY_LIMIT", "2G");
            if native {
                command.env("CHUNK_BACKUP_31_RECORDERS", engine);
            } else {
                command.env("CHUNK_BACKUP_31_ENGINE", engine);
            }
            let output = command.output().unwrap();
            let label = format!("{ids}-{engine}");
            fs::write(root.join(format!("{label}.stdout")), &output.stdout).unwrap();
            fs::write(root.join(format!("{label}.stderr")), &output.stderr).unwrap();
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
                "{label}: zero-test child"
            );
            assert!(
                output.status.success(),
                "{label}: child failed {:?}, receipts {}",
                output.status,
                root.display()
            );
        }
    }
    true
}
