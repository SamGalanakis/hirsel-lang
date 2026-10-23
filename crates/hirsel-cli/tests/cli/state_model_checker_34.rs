//! Bounded full-entry state-graph workflows, closure oracle, checked witnesses and replay.
use hirsel_driver::{native::*, replay_observations, replay_observations_checked};
use hirsel_engine::{RunConfiguration, SimulationConfig};
use hirsel_run_schema::{Access, FileReadScript};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

// Retain the actual compiler/std/runtime/linker closure before a consumer starts.
// Buck artifacts are immutable: hard links preserve them beyond sandbox cleanup.
fn tool_inputs(root: &Path) -> BTreeMap<PathBuf, String> {
    fn visit(path: &Path, files: &mut Vec<PathBuf>) {
        if path.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(&entry.unwrap().path(), files);
            }
        } else if path.is_file() {
            files.push(path.canonicalize().unwrap());
        }
    }
    let mut files = vec![
        std::env::current_exe().unwrap(),
        PathBuf::from(env!("CARGO_BIN_EXE_hirsel")),
    ];
    for name in [
        "HIRSEL_IMAGE",
        "HIRSEL_RUNTIME_ARCHIVES",
        "LLVM_SYS_231_PREFIX",
        "HIRSEL_LINK_KIT",
        "HIRSEL_GLIBC_FLOOR",
    ] {
        if let Some(path) = std::env::var_os(name) {
            visit(Path::new(&path), &mut files);
        }
    }
    for path in [
        "/usr/bin/python3",
        "/usr/bin/prlimit",
        "/usr/bin/env",
        "/usr/bin/systemd-run",
        "/usr/lib/x86_64-linux-gnu/libc.so.6",
        "/lib64/ld-linux-x86-64.so.2",
    ] {
        visit(Path::new(path), &mut files);
    }
    let blobs = root.parent().unwrap().join("tool-blobs");
    fs::create_dir_all(&blobs).unwrap();
    let mut hashes = BTreeMap::new();
    for path in files {
        if hashes.contains_key(&path) {
            continue;
        }
        let mut hash = Sha256::new();
        let mut input = fs::File::open(&path).unwrap();
        let mut buffer = [0; 65536];
        loop {
            let n = input.read(&mut buffer).unwrap();
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        let hash = format!("{:x}", hash.finalize());
        let retained = blobs.join(&hash);
        if !retained.exists() {
            match fs::hard_link(&path, &retained) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(_) => {
                    let temporary = tempfile::NamedTempFile::new_in(&blobs).unwrap();
                    fs::copy(&path, temporary.path()).unwrap();
                    if let Err(error) = temporary.persist_noclobber(&retained) {
                        assert_eq!(error.error.kind(), std::io::ErrorKind::AlreadyExists);
                    }
                }
            }
        }
        hashes.insert(path, hash);
    }
    fs::write(
        root.join("actual-inputs.json"),
        serde_json::to_vec_pretty(&hashes).unwrap(),
    )
    .unwrap();
    let config: BTreeMap<_, _> = std::env::vars()
        .filter(|(k, _)| {
            k.starts_with("HIRSEL_") || k.starts_with("LLVM_") || k.starts_with("STATECHECK_")
        })
        .collect();
    fs::write(
        root.join("actual-config.json"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
    hashes
}

fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("../support/state_model_checker_34/cases.json")).unwrap()
}
fn fault_cases() -> Vec<Value> {
    serde_json::from_str(include_str!(
        "../support/state_model_checker_34/faults.json"
    ))
    .unwrap()
}
fn native_cases() -> Vec<Value> {
    if std::env::var_os("STATECHECK_FAULTS_ONLY").is_some() {
        fault_cases()
    } else {
        cases()
    }
}
fn root(label: &str) -> PathBuf {
    // Explicit host executor is required for archival runs. Refuse an ephemeral
    // destination rather than silently claiming retained evidence from a sandbox.
    let base = Path::new("/workspace/code/hirsel-briefs/state-model-checker-34");
    fs::create_dir_all(base).expect("archive must be reachable from selected executor");
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
        .keep()
}
fn package(root: &Path) -> PathBuf {
    for (name, bytes) in [
        (
            "main.hir",
            include_str!("../../../../examples/statecheck/main.hir"),
        ),
        (
            "model.hir",
            include_str!("../../../../examples/statecheck/model.hir"),
        ),
        (
            "reach.hir",
            include_str!("../../../../examples/statecheck/reach.hir"),
        ),
        (
            "components.hir",
            include_str!("../../../../examples/statecheck/components.hir"),
        ),
        (
            "properties.hir",
            include_str!("../../../../examples/statecheck/properties.hir"),
        ),
        (
            "report.hir",
            include_str!("../../../../examples/statecheck/report.hir"),
        ),
        (
            "oracle.py",
            include_str!("../support/state_model_checker_34/oracle.py"),
        ),
        (
            "faults.json",
            include_str!("../support/state_model_checker_34/faults.json"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
        (
            "cases.json",
            include_str!("../support/state_model_checker_34/cases.json"),
        ),
    ] {
        fs::write(root.join(name), bytes).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/statecheck/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .into();
    let _ = tool_inputs(root);
    let path = root.join("hirsel.json");
    fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    path
}
fn fixture(row: &Value) -> Fixture {
    let mut data = row["input"].as_str().unwrap().as_bytes().to_vec();
    if let Some(pad) = row["pad"].as_u64() {
        data.resize(pad as usize, b' ');
    }
    let mut fixture = Fixture {
        args: vec![
            "scenario.json".into(),
            "report.txt".into(),
            row["jobs"].to_string(),
        ],
        mounts: if row["deny"] == true {
            BTreeMap::new()
        } else {
            if row["output_deny"] == true {
                BTreeMap::from([("input".into(), Access::Read)])
            } else {
                BTreeMap::from([
                    ("input".into(), Access::Read),
                    ("output".into(), Access::ReadWrite),
                ])
            }
        },
        files: BTreeMap::from([
            ("input/scenario.json".into(), data),
            ("output/report.txt".into(), b"preserve".to_vec()),
        ]),
        ..Default::default()
    };
    if row["pending"] == true {
        fixture.args.push("--interruptible".into());
        fixture
            .file_reads
            .insert("input/scenario.json".into(), FileReadScript::Pending);
        fixture.signals = Some(
            serde_json::from_value(
                json!({"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]}),
            )
            .unwrap(),
        );
    }
    if row["signal_denied"] == true {
        fixture.signals = Some(
            serde_json::from_value(
                json!({"provider":"signals.script@1","kinds":["terminate"],"script":[]}),
            )
            .unwrap(),
        );
    }
    if !row["faults"].is_null() {
        fixture.file_faults = serde_json::from_value(row["faults"].clone()).unwrap();
    }
    fixture
}
fn expected_files(row: &Value, files: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    let mut files = files.clone();
    if row["exit"] == 0 {
        files.insert(
            "output/report.txt".into(),
            row["stdout"].as_str().unwrap().as_bytes().to_vec(),
        );
    }
    files
}
fn original_steps(row: &Value) -> bool {
    ["cap-states", "cap-edges", "over-edges", "cap-input"]
        .iter()
        .any(|id| row["id"] == *id) // Exact original Steps1M witnesses; not acceptance passes.
}
fn selected(row: &Value) -> bool {
    std::env::var("STATECHECK_CASE").map_or(true, |v| v.split(',').any(|s| row["id"] == s))
}
fn child_stderr(bytes: &[u8]) -> Vec<u8> {
    bytes
        .split_inclusive(|b| *b == b'\n')
        .filter(|line| !line.starts_with(b"capped:"))
        .flatten()
        .copied()
        .collect()
}
fn check_witnesses(root: &Path, row: &Value, output: &[u8]) {
    if row["exit"] != 0 {
        return;
    }
    let check = root.join("check_output.py");
    fs::write(&check, "import json,sys\nfrom oracle import validate\ng=json.loads(sys.argv[1])['models']\nr=sys.stdin.read().split('end\\n')[:-1]\nfor a,b in zip(g,r,strict=True): validate(a,b+'end\\n')\n").unwrap();
    use std::io::Write;
    let mut child = std::process::Command::new("python3")
        .arg(check)
        .arg(row["input"].as_str().unwrap())
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(output).unwrap();
    assert!(
        child.wait().unwrap().success(),
        "independent emitted-witness oracle"
    );
}
fn assert_record(bytes: &[u8], row: &Value) {
    let record = hirsel_rt::launch::parse_recording_bytes(bytes).unwrap();
    let stderr: Vec<u8> = record
        .events
        .iter()
        .filter_map(|event| match &event.request {
            hirsel_rt::io::Request::PrintStderr { bytes, .. } => Some(bytes.as_slice()),
            _ => None,
        })
        .flatten()
        .copied()
        .collect();
    assert_eq!(stderr, row["stderr"].as_str().unwrap().as_bytes());
    let publications: Vec<_> = record
        .events
        .iter()
        .filter(|e| {
            matches!(
                &e.request,
                hirsel_rt::io::Request::CreateAtomic { .. }
                    | hirsel_rt::io::Request::WriteAll { .. }
                    | hirsel_rt::io::Request::Commit { .. }
            )
        })
        .collect();
    if row["exit"] == 0 {
        assert_eq!(
            publications.len(),
            3,
            "one atomic publication, no leaked temporary"
        );
        assert!(
            matches!(&publications[0].request, hirsel_rt::io::Request::CreateAtomic { path, .. } if path == "report.txt")
        );
        assert!(
            matches!(&publications[1].request, hirsel_rt::io::Request::WriteAll { bytes, .. } if bytes == row["stdout"].as_str().unwrap().as_bytes())
        );
        assert!(matches!(
            (&publications[2].request, &publications[2].completion),
            (
                hirsel_rt::io::Request::Commit { .. },
                hirsel_rt::io::Completion::Unit(Ok(()))
            )
        ));
    } else if !row["faults"].is_null() {
        assert!(
            record.events.iter().any(|e| e.fault.is_some()),
            "the scripted fault was actually consumed"
        );
        assert!(
            !publications.iter().any(|e| matches!(
                (&e.request, &e.completion),
                (
                    hirsel_rt::io::Request::Commit { .. },
                    hirsel_rt::io::Completion::Unit(Ok(()))
                )
            )),
            "failed publication cannot commit"
        );
    } else {
        assert!(
            publications.is_empty(),
            "refused workflow must preserve output"
        );
    }
    if row["pending"] == true {
        assert!(record.events.iter().any(|e| matches!(
            (&e.request, &e.completion),
            (
                hirsel_rt::io::Request::FileRead { .. },
                hirsel_rt::io::Completion::Blocked { committed: false }
            )
        )));
    }
}
#[test]
fn state_model_checker_34_frozen_oracle_vectors() {
    let rows = cases();
    assert_eq!(rows.len(), 46);
    assert!(
        rows.iter()
            .any(|r| r["id"] == "absorbing"
                && r["stdout"].as_str().unwrap().contains("safety unsafe"))
    );
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::write(
        root.join("oracle.py"),
        include_str!("../support/state_model_checker_34/oracle.py"),
    )
    .unwrap();
    fs::write(
        root.join("cases.json"),
        include_str!("../support/state_model_checker_34/cases.json"),
    )
    .unwrap();
    let result = std::process::Command::new("python3")
        .arg(root.join("oracle.py"))
        .arg("--check")
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
}
#[test]
fn state_model_checker_34_reference_checked_workflows() {
    hirsel_engine::with_compiler_stack(|| {
        let root = root("simulation-");
        let path = package(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut receipts = Vec::new();
        let mut failures = Vec::new();
        for row in cases().iter().filter(|r| selected(r)) {
            let recipe = fixture(row);
            let world = SimulationConfig::from(&recipe);
            let mut prior = None;
            for checked in [false, true] {
                let engine = if checked { "checked-h" } else { "reference" };
                eprintln!("STATECHECK_STARTED {} {engine}", row["id"]);
                let result = if checked {
                    hirsel_engine::run_observed_checked_bounded(
                        &mut loaded.engine,
                        &run,
                        world.clone(),
                        1_000_000,
                    )
                } else {
                    hirsel_engine::run_observed_snapshot_bounded(
                        &mut loaded.engine,
                        &run,
                        world.clone(),
                        1_000_000,
                    )
                };
                let result = result.map_err(|e| e.to_string()).and_then(|execution| {
                    if original_steps(row) {
                        assert!(execution.observation.is_none());
                        assert!(matches!(&execution.outcome, Ok(hirsel_engine::RunOutcome::ToolingLimit(limit)) if limit.kind == "Steps" && limit.limit == 1_000_000));
                        return Err(format!("INCOMPLETE original Steps1M: {:?}",execution.outcome));
                    }
                    let observed = execution.observation.ok_or_else(|| format!("INCOMPLETE {:?}", execution.outcome))?;
                    if observed.exit_code != row["exit"].as_i64().map(|n| n as i32) || observed.output != row["stdout"].as_str().unwrap().as_bytes() || !observed.failures.is_empty() || observed.blocked.is_some() {
                        return Err(format!("unexpected outcome {:?}, stdout {:?}, failures {:?}", execution.outcome, String::from_utf8_lossy(&observed.output), observed.failures));
                    }
                    check_witnesses(&root, row, &observed.output);
                    let state = observed.final_state.as_ref().unwrap();
                    assert_eq!(state.stderr, row["stderr"].as_str().unwrap().as_bytes());
                    assert_eq!(state.trees[&0].1, expected_files(row, &world.files), "atomic publication and temporary cleanup");
                    if row["pending"] == true {
                        assert!(observed.effects.iter().any(|e| matches!(&e.effect, hirsel_interp::observation::Effect::CapabilityConsumed(e) if e.operation == "file.read.pending")), "never parked");
                    }
                    if let Some(prior) = &prior { assert_eq!(&observed, prior); }
                    prior = Some(observed);
                    Ok(())
                });
                if let Err(error) = &result
                    && !original_steps(row)
                {
                    failures.push(format!("{}/{engine}: {error}", row["id"]));
                }
                receipts.push(json!({"case":row["id"],"engine":engine,"started":true,"passed":result.is_ok(),"error":result.err()}));
                fs::write(
                    root.join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
                eprintln!(
                    "STATECHECK_RESULT {} {engine} {}",
                    row["id"],
                    receipts.last().unwrap()
                );
            }
        }
        assert!(!receipts.is_empty());
        assert!(
            failures.is_empty(),
            "{}: {}",
            root.display(),
            failures.join("\n")
        );
    });
}
#[test]
fn state_model_checker_34_native_full_entry_replay() {
    hirsel_engine::with_compiler_stack(|| {
        let root = std::env::var_os("STATECHECK_NATIVE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| root("native-"));
        let path = if root.join("hirsel.json").exists() {
            root.join("hirsel.json")
        } else {
            package(&root)
        };
        let recovery = root.join(format!("consumer-{}", std::process::id()));
        fs::create_dir_all(&recovery).unwrap();
        let _ = tool_inputs(&recovery);
        let receipts_path = root.join("receipts.json");
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
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
        tools.launch_timeout = std::time::Duration::from_secs(70);
        tools.execution = Execution::Executable;
        tools.wrapper = vec![
            "/usr/bin/env".into(),
            "PATH=/usr/bin:/bin".into(),
            "HIRSEL_MEMORY_LIMIT=2G".into(),
            "/usr/bin/python3".into(),
            root.join("capped").to_string_lossy().into_owned(),
            "--timeout".into(),
            "60".into(),
            "/usr/bin/prlimit".into(),
            "--cpu=60:61".into(),
        ];
        let ready = build(&input, &options).unwrap();
        let sites = Sites::new(input.snapshot());
        let objects: Vec<_> = [Backend::Llvm, Backend::Cranelift]
            .into_iter()
            .map(|backend| {
                (
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
                )
            })
            .collect();
        let mut receipts: Vec<Value> = if receipts_path.exists() {
            serde_json::from_slice(&fs::read(&receipts_path).unwrap()).unwrap()
        } else {
            Vec::new()
        };
        let raw_only = std::env::var_os("STATECHECK_RAW_STDERR_ONLY").is_some();
        for row in native_cases()
            .iter()
            .filter(|r| selected(r) && (!raw_only || r["stderr"] != ""))
        {
            let fixture = fixture(row);
            let expected = Observation {
                exit_code: row["exit"].as_i64().map(|n| n as i32),
                output: row["stdout"].as_str().unwrap().as_bytes().to_vec(),
                blocked: false,
                budget: None,
                signaled: None,
                failures: vec![],
                roots: vec![],
            };
            let case_dir = root.join(row["id"].as_str().unwrap());
            fs::create_dir_all(&case_dir).unwrap();
            let expected_cells = 2;
            if !raw_only
                && receipts
                    .iter()
                    .filter(|r| r["case"] == row["id"] && r["outcome"] == "Agree")
                    .count()
                    < expected_cells
            {
                for (name, outcome) in differential_in(
                    &input,
                    &loaded.entry,
                    &expected,
                    &matrix,
                    &tools,
                    &case_dir,
                    &fixture,
                ) {
                    receipts.push(
                        json!({"case":row["id"],"engine":name,"outcome":format!("{outcome:?}")}),
                    );
                    fs::write(
                        root.join("receipts.json"),
                        serde_json::to_vec_pretty(&receipts).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(outcome, Outcome::Agree, "{} {name}", row["id"]);
                }
            }
            let mut runnables = Vec::new();
            for (backend, objects) in &objects {
                let dir = case_dir.join(format!("{backend:?}"));
                fs::create_dir_all(&dir).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &loaded.entry, objects, &tools, &dir, &fixture)
                        .unwrap();
                runnables.push((runnable, dir));
            }
            for (runnable, dir) in &runnables {
                if raw_only {
                    let raw = launch_fixture(
                        runnable,
                        &ready.target,
                        &tools,
                        dir,
                        true,
                        Tape::Live {
                            record: None,
                            revocations: None,
                        },
                        &fixture,
                    )
                    .unwrap();
                    fs::write(dir.join("raw-live.stdout"), &raw.stdout).unwrap();
                    fs::write(dir.join("raw-live.stderr"), &raw.stderr).unwrap();
                    fs::write(dir.join("raw-live.observation"), &raw.record).unwrap();
                    assert_eq!(observe(&raw, &ready, &sites).unwrap(), expected);
                    assert_eq!(
                        child_stderr(&raw.stderr),
                        row["stderr"].as_str().unwrap().as_bytes(),
                        "exact raw live stderr obligation"
                    );
                    receipts.push(
                        json!({"case":row["id"],"dir":dir,"raw_live_stderr":true,"passed":true}),
                    );
                    fs::write(
                        &receipts_path,
                        serde_json::to_vec_pretty(&receipts).unwrap(),
                    )
                    .unwrap();
                    continue;
                }
                let tape = dir.join("recording");
                if !dir.join("record-passed").exists() {
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
                    assert_eq!(observe(&recorded, &ready, &sites).unwrap(), expected);
                    fs::write(dir.join("raw-record.stderr"), &recorded.stderr).unwrap();
                    fs::write(dir.join("raw-record.stdout"), &recorded.stdout).unwrap();
                    assert_eq!(
                        child_stderr(&recorded.stderr),
                        row["stderr"].as_str().unwrap().as_bytes(),
                        "exact raw live stderr"
                    );
                    fs::write(
                        dir.join("record-passed"),
                        b"complete semantic observation and stderr verified",
                    )
                    .unwrap();
                }
                let frozen = fs::read(&tape).unwrap();
                assert_record(&frozen, row);
                let replay_fixture = Fixture {
                    files: BTreeMap::new(),
                    file_reads: BTreeMap::new(),
                    ..fixture.clone()
                };
                if !receipts
                    .iter()
                    .any(|r| r["tape"] == tape.to_string_lossy().as_ref() && r["record"] == true)
                {
                    for (index, (replayer, replay_dir)) in runnables.iter().enumerate() {
                        if receipts.iter().any(|r| {
                            r["tape"] == tape.to_string_lossy().as_ref()
                                && r["native_replay"] == index
                                && r["passed"] == true
                        }) {
                            continue;
                        }
                        let replayed = launch_fixture(
                            replayer,
                            &ready.target,
                            &tools,
                            replay_dir,
                            true,
                            Tape::Replay(&tape),
                            &replay_fixture,
                        )
                        .unwrap();
                        assert_eq!(observe(&replayed, &ready, &sites).unwrap(), expected);
                        assert_eq!(
                            child_stderr(&replayed.stderr),
                            row["stderr"].as_str().unwrap().as_bytes()
                        );
                        receipts.push(json!({"case":row["id"],"tape":tape,"native_replay":index,"passed":true}));
                        fs::write(
                            &receipts_path,
                            serde_json::to_vec_pretty(&receipts).unwrap(),
                        )
                        .unwrap();
                    }
                    for (index, replay) in [replay_observations, replay_observations_checked]
                        .into_iter()
                        .enumerate()
                    {
                        if receipts.iter().any(|r| {
                            r["tape"] == tape.to_string_lossy().as_ref()
                                && r["interpreter_replay_engine"] == index
                                && r["passed"] == true
                        }) {
                            continue;
                        }
                        if original_steps(row) {
                            let error = replay(&path, &tape, Some(1_000_000))
                                .unwrap_err()
                                .to_string();
                            assert!(
                                error.contains("ToolingLimit")
                                    && error.contains("Steps")
                                    && error.contains("limit: 1000000"),
                                "{error}"
                            );
                            receipts.push(json!({"case":row["id"],"interpreter_replay":"INCOMPLETE Steps1M","passed":false,"error":error}));
                        } else {
                            let observed = replay(&path, &tape, Some(1_000_000)).unwrap();
                            assert_eq!(Observation::of_reference(&observed), expected);
                            receipts.push(json!({"case":row["id"],"tape":tape,"interpreter_replay_engine":index,"passed":true}));
                            fs::write(
                                &receipts_path,
                                serde_json::to_vec_pretty(&receipts).unwrap(),
                            )
                            .unwrap();
                            // Host replay has no simulated final_state. Exact stderr,
                            // atomic output bytes and commit are checked by assert_record.
                        }
                    }
                    receipts.push(json!({"case":row["id"],"tape":tape,"record":true,"cross_backend_replays":2,"interpreter_replays":if original_steps(row) {0} else {2}}));
                    fs::write(
                        &receipts_path,
                        serde_json::to_vec_pretty(&receipts).unwrap(),
                    )
                    .unwrap();
                }
                if row["id"] == "self-loop" {
                    let recording = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
                    for kind in ["binding", "request", "missing-event", "extra-event"] {
                        if receipts.iter().any(|r| {
                            r["control"] == kind
                                && r["engines"] == 4
                                && r["tape"] == tape.to_string_lossy().as_ref()
                        }) {
                            continue;
                        }
                        let mut changed = recording.clone();
                        let diagnostic = match kind {
                            "binding" => {
                                changed.arguments.push("changed".into());
                                "header disagrees"
                            }
                            "request" => {
                                let request = changed
                                    .events
                                    .iter_mut()
                                    .find_map(|e| match &mut e.request {
                                        hirsel_rt::io::Request::Directory { path, .. } => {
                                            Some(path)
                                        }
                                        _ => None,
                                    })
                                    .unwrap();
                                request.push_str("-changed");
                                "ReplayMismatch"
                            }
                            "missing-event" => {
                                changed.events.pop().unwrap();
                                "MissingInput"
                            }
                            _ => {
                                changed.events.push(recording.events[0].clone());
                                "UnconsumedInput"
                            }
                        };
                        let bad = dir.join(format!("{kind}.tape"));
                        fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap())
                            .unwrap();
                        for checked in [false, true] {
                            let engine = if checked { "checked-h" } else { "reference" };
                            if receipts.iter().any(|r| {
                                r["control"] == kind
                                    && r["control_engine"] == engine
                                    && r["tape"] == tape.to_string_lossy().as_ref()
                            }) {
                                continue;
                            }
                            let error = if checked {
                                replay_observations_checked(&path, &bad, Some(1_000_000))
                            } else {
                                replay_observations(&path, &bad, Some(1_000_000))
                            }
                            .unwrap_err()
                            .to_string();
                            assert!(error.contains(diagnostic), "{kind}: {error}");
                            receipts.push(json!({"case":row["id"],"control":kind,"control_engine":engine,"tape":tape,"passed":true}));
                            fs::write(
                                &receipts_path,
                                serde_json::to_vec_pretty(&receipts).unwrap(),
                            )
                            .unwrap();
                        }
                        for (index, (replayer, replay_dir)) in runnables.iter().enumerate() {
                            let engine = format!("native-{index}");
                            if receipts.iter().any(|r| {
                                r["control"] == kind
                                    && r["control_engine"] == engine
                                    && r["tape"] == tape.to_string_lossy().as_ref()
                            }) {
                                continue;
                            }
                            match launch_fixture(
                                replayer,
                                &ready.target,
                                &tools,
                                replay_dir,
                                true,
                                Tape::Replay(&bad),
                                &replay_fixture,
                            ) {
                                Err(error) => {
                                    assert!(error.message.contains(diagnostic), "{kind}: {error}")
                                }
                                Ok(refused) => {
                                    assert!(
                                        !refused.record.is_empty(),
                                        "public launcher must retain refusal"
                                    );
                                    fs::write(
                                        replay_dir.join(format!("{kind}-retained-refusal")),
                                        &refused.record,
                                    )
                                    .unwrap();
                                    assert_eq!(
                                        refused.exit,
                                        Some(3),
                                        "{kind}: status {:?}",
                                        refused.exit
                                    );
                                    assert!(
                                        observe(&refused, &ready, &sites)
                                            .unwrap_err()
                                            .to_string()
                                            .contains(diagnostic),
                                        "{kind}: status {:?}",
                                        refused.exit
                                    );
                                    assert!(
                                        refused.stdout
                                            == if kind == "extra-event" {
                                                expected.output.clone()
                                            } else {
                                                Vec::new()
                                            },
                                        "invalid early/late stdout prefix"
                                    );
                                }
                            }
                            receipts.push(json!({"case":row["id"],"control":kind,"control_engine":engine,"tape":tape,"passed":true}));
                            fs::write(
                                &receipts_path,
                                serde_json::to_vec_pretty(&receipts).unwrap(),
                            )
                            .unwrap();
                        }
                        receipts.push(json!({"case":row["id"],"control":kind,"tape":tape,"engines":4,"passed":true}));
                        fs::write(
                            &receipts_path,
                            serde_json::to_vec_pretty(&receipts).unwrap(),
                        )
                        .unwrap();
                    }
                }
                assert_eq!(fs::read(&tape).unwrap(), frozen);

                fs::write(
                    root.join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
            }
        }
        assert!(!receipts.is_empty());
    });
}

#[test]
fn state_model_checker_34_arithmetic_and_encoder_boundaries() {
    let root = root("helpers-");
    let path = package(&root);
    for backend in ["llvm", "cranelift"] {
        let output = std::process::Command::new("python3")
            .arg(root.join("capped"))
            .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
            .arg(env!("CARGO_BIN_EXE_hirsel"))
            .args(["test", "--native", "--backend", backend])
            .arg(&path)
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .output()
            .unwrap();
        fs::write(root.join(format!("{backend}.stdout")), &output.stdout).unwrap();
        fs::write(root.join(format!("{backend}.stderr")), &output.stderr).unwrap();
        assert!(output.status.success(), "{backend}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "zero helper tests: {output:?}"
        );
    }
}

// Ordered capacity remains an ignored owner-blocked obligation. These cells
// use the same public Fixture and full main, with supported placement requests.
// Missing running replies fail explicitly, even when semantic output agrees.
fn ordered(backend: Backend) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    hirsel_engine::with_compiler_stack(|| {
        let root = root(&format!("ordered-{backend:?}-"));
        let path = package(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let matrix = vec![Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        }];
        let mut tools = Toolchain::for_matrix(&root.join("runtime"), &matrix).unwrap();
        tools.execution = Execution::Executable;
        tools.launch_timeout = std::time::Duration::from_secs(60);
        let ready = build(&input, &options).unwrap();
        let sites = Sites::new(input.snapshot());
        let objects = emit_backend_objects(
            &ready,
            backend,
            OptLevel::None,
            Partition::Whole,
            false,
            None,
        )
        .unwrap();
        let rows = cases();
        let mut receipts = Vec::new();
        for (mode, id) in [
            ("success", "four-models"),
            ("domain", "duplicate-edge"),
            ("cancel", "pending-cancel"),
        ] {
            let row = rows.iter().find(|r| r["id"] == id).unwrap();
            let fixture = fixture(row);
            let mut previous = None;
            for width in [1, 2, 4] {
                let dir = root.join(format!("{mode}-w{width}"));
                fs::create_dir(&dir).unwrap();
                let short = tempfile::tempdir().unwrap();
                let control = short.path().join("ctl");
                // First bind through the documented Fixture API, then re-link the
                // same full entry with the public validated placement request.
                let _ =
                    link_fixture_runnable(&ready, &loaded.entry, &objects, &tools, &dir, &fixture)
                        .unwrap();
                let bound = hirsel_run_config::RunConfig::read_path(&dir.join("run.json")).unwrap();
                let mut record = bound.record().clone();
                record.placement=Some(hirsel_run_config::Placement::from_json(&serde_json::to_vec(&json!({"cores":width,"commit":"shared","reorder":"off","control":control})).unwrap()).unwrap());
                let requested = record.validated().unwrap();
                let runnable = link_runnable(
                    &ready,
                    &loaded.entry,
                    &objects,
                    &tools,
                    &dir,
                    Some(&requested),
                )
                .unwrap();
                let tape = dir.join("recording");
                let mut replies = Vec::new();
                let launched = std::thread::scope(|scope| {
                    let child = scope.spawn(|| {
                        launch_fixture(
                            &runnable,
                            &ready.target,
                            &tools,
                            &dir,
                            true,
                            Tape::Live {
                                record: Some(&tape),
                                revocations: None,
                            },
                            &fixture,
                        )
                    });
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while !child.is_finished() && std::time::Instant::now() < deadline {
                        if let Ok(mut connection) = UnixStream::connect(&control) {
                            connection
                                .set_read_timeout(Some(std::time::Duration::from_millis(100)))
                                .unwrap();
                            if connection.write_all(b"{\"metrics\":true}\n").is_ok() {
                                let mut reply = String::new();
                                if BufReader::new(connection).read_line(&mut reply).is_ok()
                                    && let Ok(value) = serde_json::from_str::<Value>(&reply)
                                {
                                    replies.push(value);
                                }
                            }
                        }
                        if replies.iter().any(|m| {
                            m["worker_phase"] == "running"
                                && m["width"]["requested"] == width
                                && m["width"]["effective"] == width
                                && m["width"]["pending"] == 0
                        }) {
                            break;
                        }
                        std::thread::yield_now();
                    }
                    child.join().unwrap().unwrap()
                });
                let capacity = replies.iter().any(|m| {
                    m["worker_phase"] == "running"
                        && m["width"]["requested"] == width
                        && m["width"]["effective"] == width
                        && m["width"]["pending"] == 0
                });
                receipts.push(json!({"mode":mode,"requested":width,"control":replies,"capacity":capacity,"completed":true}));
                fs::write(
                    root.join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
                assert!(
                    capacity,
                    "{}: exact running capacity reply missing",
                    dir.display()
                );
                let observed = observe(&launched, &ready, &sites).unwrap();
                assert_eq!(observed.exit_code, row["exit"].as_i64().map(|n| n as i32));
                assert_eq!(observed.output, row["stdout"].as_str().unwrap().as_bytes());
                assert!(observed.failures.is_empty());
                assert_record(&fs::read(&tape).unwrap(), row);
                let frozen = fs::read(&tape).unwrap();
                let mut semantic = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
                let binding = semantic.binding.as_mut().unwrap();
                binding.admitted_at = 0;
                binding.placement = None;
                if let Some(previous) = &previous {
                    assert_eq!(
                        &semantic, previous,
                        "full scripted semantic events differ by width"
                    );
                } else {
                    previous = Some(semantic);
                }
                let replay_fixture = Fixture {
                    files: BTreeMap::new(),
                    file_reads: BTreeMap::new(),
                    ..fixture.clone()
                };
                let replayed = launch_fixture(
                    &runnable,
                    &ready.target,
                    &tools,
                    &dir,
                    true,
                    Tape::Replay(&tape),
                    &replay_fixture,
                )
                .unwrap();
                assert_eq!(observe(&replayed, &ready, &sites).unwrap(), observed);
                assert_eq!(fs::read(&tape).unwrap(), frozen);
            }
        }
    });
}
#[test]
#[ignore = "exact CPU deploy owned by @agentlang-24; capacity requires running control replies"]
fn state_model_checker_34_ordered_llvm() {
    ordered(Backend::Llvm);
}
#[test]
#[ignore = "exact CPU deploy owned by @agentlang-24; capacity requires running control replies"]
fn state_model_checker_34_ordered_cranelift() {
    ordered(Backend::Cranelift);
}

#[test]
fn state_model_checker_34_file_faults_reference_checked() {
    hirsel_engine::with_compiler_stack(|| {
        let root = root("fault-simulation-");
        let path = package(&root);
        let mut loaded = hirsel_driver::load_program(&path).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut receipts = Vec::new();
        for row in fault_cases() {
            let recipe = fixture(&row);
            let world = SimulationConfig::from(&recipe);
            let mut previous = None;
            for checked in [false, true] {
                let execution = if checked {
                    hirsel_engine::run_observed_checked_bounded(
                        &mut loaded.engine,
                        &run,
                        world.clone(),
                        1_000_000,
                    )
                } else {
                    hirsel_engine::run_observed_snapshot_bounded(
                        &mut loaded.engine,
                        &run,
                        world.clone(),
                        1_000_000,
                    )
                }
                .unwrap_or_else(|e| panic!("{e}"));
                let observed = execution
                    .observation
                    .unwrap_or_else(|| panic!("INCOMPLETE {:?}", execution.outcome));
                assert_eq!(observed.exit_code, Some(2));
                assert!(
                    observed.output.is_empty()
                        && observed.failures.is_empty()
                        && observed.blocked.is_none()
                );
                let state = observed.final_state.as_ref().unwrap();
                assert_eq!(state.stderr, row["stderr"].as_str().unwrap().as_bytes());
                assert_eq!(
                    state.trees[&0].1, world.files,
                    "fault must preserve old target and release temporary"
                );
                if let Some(previous) = &previous {
                    assert_eq!(&observed, previous);
                }
                previous = Some(observed);
                receipts.push(json!({"case":row["id"],"engine":if checked {"checked-h"} else {"reference"},"passed":true}));
                fs::write(
                    root.join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
            }
        }
        assert_eq!(receipts.len(), 10);
    });
}
