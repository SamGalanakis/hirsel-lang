//! Full six-module application workflows. Independent Python oracle supplies vectors.
use hirsel_driver::native::*;
use hirsel_engine::{
    RunConfiguration, run_observed_checked_bounded, run_observed_snapshot_bounded,
};
use hirsel_run_schema::{Access, FileReadScript};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const SOURCES: [(&str, &str); 6] = [
    (
        "main",
        include_str!("../../../../examples/toyanalyze/main.hir"),
    ),
    (
        "syntax",
        include_str!("../../../../examples/toyanalyze/syntax.hir"),
    ),
    (
        "symbols",
        include_str!("../../../../examples/toyanalyze/symbols.hir"),
    ),
    (
        "analyze",
        include_str!("../../../../examples/toyanalyze/analyze.hir"),
    ),
    (
        "incremental",
        include_str!("../../../../examples/toyanalyze/incremental.hir"),
    ),
    (
        "report",
        include_str!("../../../../examples/toyanalyze/report.hir"),
    ),
];
struct Workspace(PathBuf);
impl Workspace {
    fn path(&self) -> &Path {
        &self.0
    }
    fn keep(self) -> PathBuf {
        self.0
    }
}
fn scratch() -> Workspace {
    let base = std::env::var_os("TOY_ANALYZER_31_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("toy-analyzer-31-")
        .tempdir_in(base)
        .unwrap()
        .keep();
    eprintln!("TOY_WORKSPACE {}", directory.display());
    Workspace(directory)
}
fn package(root: &Path) -> PathBuf {
    for (name, source) in SOURCES {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/toyanalyze/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    let path = root.join("hirsel.json");
    fs::write(&path, manifest.to_string()).unwrap();
    let seal_script = root.join("runtime_seal.py");
    fs::write(
        &seal_script,
        include_str!("../support/toy_analyzer_31/runtime_seal.py"),
    )
    .unwrap();
    let sealed = Command::new("python3")
        .arg(&seal_script)
        .arg(root.join("runtime-seal.json"))
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg(std::env::current_exe().unwrap())
        .output()
        .unwrap();
    assert!(
        sealed.status.success(),
        "{}",
        String::from_utf8_lossy(&sealed.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&sealed.stdout));
    path
}
fn cases() -> Vec<Value> {
    serde_json::from_str(include_str!("../support/toy_analyzer_31/cases.json")).unwrap()
}
fn fixture(case: &Value, jobs: u64) -> Fixture {
    let modules = case["modules"].as_object().unwrap();
    let declarations: Vec<_> = modules
        .keys()
        .map(|name| json!({"name":name,"path":format!("{name}.toy")}))
        .collect();
    let mut files: BTreeMap<String, Vec<u8>> = modules
        .iter()
        .map(|(name, source)| {
            (
                format!("input/{name}.toy"),
                source.as_str().unwrap().as_bytes().to_vec(),
            )
        })
        .collect();
    files.insert(
        "input/modules.json".into(),
        json!({"modules":declarations}).to_string().into_bytes(),
    );
    Fixture {
        args: vec!["check".into(), "modules.json".into(), jobs.to_string()],
        files,
        mounts: BTreeMap::from([
            ("input".into(), Access::Read),
            ("output".into(), Access::ReadWrite),
        ]),
        ..Default::default()
    }
}
fn rows(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8(bytes.to_vec())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn assert_case(case: &Value, observation: &hirsel_interp::observation::Observable) {
    assert!(
        observation.failures.is_empty() && observation.blocked.is_none(),
        "{observation:?}"
    );
    assert_eq!(
        observation.exit_code,
        Some(case["exit"].as_i64().unwrap() as i32),
        "{}: {observation:?}",
        case["id"]
    );
    let mut actual = rows(&observation.output).pop().unwrap();
    actual.as_object_mut().unwrap().remove("analyzed");
    actual.as_object_mut().unwrap().remove("reused");
    assert_eq!(actual, case["expected"], "{}", case["id"]);
    assert!(observation.final_state.as_ref().unwrap().stderr.is_empty());
}
fn run_case(
    loaded: &mut hirsel_engine::LoadedProgram,
    fixture: &Fixture,
    id: &str,
    steps: u64,
) -> hirsel_interp::observation::Observable {
    let run = RunConfiguration {
        entry: loaded.entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut previous = None;
    for checked in [false, true] {
        eprintln!(
            "TOY_STARTED {id} {} requested_steps={steps}",
            if checked { "checked-h" } else { "reference" }
        );
        let execution = if checked {
            run_observed_checked_bounded(&mut loaded.engine, &run, fixture.into(), steps)
        } else {
            run_observed_snapshot_bounded(&mut loaded.engine, &run, fixture.into(), steps)
        }
        .unwrap();
        eprintln!(
            "TOY_USAGE {id} checked={checked} {:?} outcome={:?}",
            execution.step_usage, execution.outcome
        );
        let observed = execution
            .observation
            .unwrap_or_else(|| panic!("INCOMPLETE {id}: {:?}", execution.outcome));
        if let Some(previous) = &previous {
            assert_eq!(&observed, previous, "{id}");
        }
        previous = Some(observed);
        eprintln!("TOY_COMPLETED {id} checked={checked}");
    }
    previous.unwrap()
}
#[test]
fn toy_analyzer_31_reference_checked_oracle_and_session() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch();
        let path = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let selected: Vec<_> = cases()
            .into_iter()
            .filter(|c| {
                std::env::var("TOY_ANALYZER_31_CASE")
                    .map_or(true, |f| f.split(',').any(|id| c["id"] == id))
            })
            .collect();
        assert!(!selected.is_empty());
        for case in &selected {
            for jobs in [1, 2, 3, 4] {
                let id = format!("{}-jobs{jobs}", case["id"].as_str().unwrap());
                let observed = run_case(
                    &mut loaded,
                    &fixture(case, jobs),
                    &id,
                    case["steps"].as_u64().unwrap(),
                );
                assert_case(case, &observed);
                eprintln!("TOY_PASS {id} reference checked-h");
            }
        }
        let case =
            json!({"modules":{"a":"let n=1;","b":"import a; let n=a.n+1;","c":"let n=true;"}});
        let mut world = fixture(&case, 2);
        world.args = vec![
            "session".into(),
            "modules.json".into(),
            "2".into(),
            "session.json".into(),
        ];
        world.files.insert(
            "input/session.json".into(),
            json!({"commands":[
                {"check":true},
                {"replace":{"module":"a","source":"let n=1; "}},{"check":true},
                {"replace":{"module":"a","source":"let n=true;"}},{"check":true},
                {"replace":{"module":"b","source":"let n=a.n+1;"}},{"check":true}
            ]})
            .to_string()
            .into_bytes(),
        );
        let observed = run_case(&mut loaded, &world, "edit-session", 1_000_000);
        let output = rows(&observed.output);
        assert_eq!(output.len(), 5);
        assert_eq!(output[1]["analyzed"], json!([]));
        assert_eq!(output[1]["reused"], json!(["a", "b", "c"]));
        assert_eq!(output[2]["analyzed"], json!(["a", "b"]));
        assert_eq!(output[2]["reused"], json!(["c"]));
        assert_eq!(
            output[3]["diagnostics"],
            json!([{"module":"b","code":"type_mismatch","start":19,"end":20}])
        );
        assert_eq!(
            output[4]["diagnostics"],
            json!([{"module":"b","code":"undefined","start":6,"end":9}])
        );
        // Independent clean app execution after each edited content version.
        for (i, a, b) in [
            (2, "let n=1; ", "import a; let n=a.n+1;"),
            (3, "let n=true;", "import a; let n=a.n+1;"),
            (4, "let n=true;", "let n=a.n+1;"),
        ] {
            let clean = fixture(&json!({"modules":{"a":a,"b":b,"c":"let n=true;"}}), 2);
            let rebuilt = run_case(&mut loaded, &clean, &format!("clean-{i}"), 1_000_000);
            let row = rows(&rebuilt.output).pop().unwrap();
            assert_eq!(row["diagnostics"], output[i]["diagnostics"]);
            assert_eq!(row["total"], output[i]["total"]);
        }
    });
}
#[test]
fn toy_analyzer_31_caps_denial_and_pending_cancellation() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch();
        let path = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let base = json!({"modules":{"a":"let x=1;"}});
        let mut tests = vec![];
        for (id, source, expected) in [
            ("bytes8192", format!("let x=1;{}", " ".repeat(8184)), None),
            (
                "bytes8193",
                format!("let x=1;{}", " ".repeat(8185)),
                Some("input_bytes"),
            ),
            (
                "bindings256",
                (0..256).map(|i| format!("let x{i}=1;")).collect::<String>(),
                None,
            ),
            (
                "bindings257",
                (0..257).map(|i| format!("let x{i}=1;")).collect::<String>(),
                Some("bindings"),
            ),
            (
                "depth24",
                format!("{}let x=1;{}", "{".repeat(23), "}".repeat(23)),
                None,
            ),
            (
                "depth25",
                format!("{}let x=1;{}", "{".repeat(24), "}".repeat(24)),
                Some("ast_depth"),
            ),
        ] {
            let world = fixture(&json!({"modules":{"a":source}}), 2);
            tests.push((id.to_string(), world, expected));
        }
        for n in [16, 17] {
            let modules: serde_json::Map<String, Value> =
                (0..n).map(|i| (format!("m{i}"), json!(""))).collect();
            tests.push((
                format!("modules{n}"),
                fixture(&json!({"modules":modules}), 2),
                if n == 17 { Some("modules") } else { None },
            ));
        }
        tests.push(("jobs5".into(), fixture(&base, 5), Some("jobs")));
        let mut denied = fixture(&base, 2);
        denied.mounts.clear();
        tests.push(("denied".into(), denied, Some("file")));
        let mut pending = fixture(&json!({"modules":{"a":"let x=1;","b":""}}), 2);
        pending.files.insert(
            "input/b.toy".into(),
            format!("{}let x=1;{}", "{".repeat(24), "}".repeat(24)).into_bytes(),
        );
        pending
            .file_reads
            .insert("input/a.toy".into(), FileReadScript::Pending);
        tests.push(("pending-cancel".into(), pending, Some("ast_depth")));
        for (id, world, error) in tests {
            // Fixed 20M large-fixture budget declared before first execution.
            let observed = run_case(&mut loaded, &world, &id, 20_000_000);
            if let Some(error) = error {
                assert_eq!(
                    rows(&observed.output).last().unwrap(),
                    &json!({"error":error}),
                    "{id}"
                );
                assert_eq!(observed.exit_code, Some(2));
            } else {
                assert_eq!(observed.exit_code, Some(0), "{id}: {observed:?}");
            }
            if id == "pending-cancel" {
                assert!(observed.effects.iter().any(|e| matches!(&e.effect,hirsel_interp::observation::Effect::CapabilityConsumed(c) if c.operation=="file.read.pending")),"no actual pending read");
                assert!(observed.effects.iter().any(|e| matches!(&e.effect,hirsel_interp::observation::Effect::CapabilityConsumed(c) if c.operation=="file.close")),"no cleanup close");
            }
            eprintln!("TOY_PASS {id} reference checked-h");
        }
    });
}
#[test]
fn toy_analyzer_31_scripted_native_and_four_engine_replay() {
    scripted_native(0)
}
#[test]
fn toy_analyzer_31_remaining_native_cache_and_session() {
    scripted_native(1)
}
fn scripted_native(group: u8) {
    let remaining = group == 1;
    let readiness = group == 2;
    let missing = group == 3;
    let widths = group == 4;
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch();
        let path = package(scratch.path());
        // Every test owns an initially empty ordinary store. Each replay still
        // opens a fresh compiler, using the same policy as repeated CLI runs.
        let compiler = hirsel_driver::COMPILER
            .clone()
            .with_config(hirsel_engine::CompileConfig {
                store: hirsel_engine::store::store_by_name(
                    "dir@1",
                    Some(scratch.path().join("products")),
                )
                .unwrap(),
                ..hirsel_driver::COMPILER.configuration()
            });
        let mut loaded = compiler.load_program(&path).unwrap();
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
        let tools = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
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
        let mut selected: Vec<_> = cases()
            .into_iter()
            .filter(|c| {
                !readiness
                    && !missing
                    && remaining
                        != ["integer", "undefined", "chain"].contains(&c["id"].as_str().unwrap())
            })
            .map(|c| {
                let f = fixture(&c, 2);
                (c["id"].as_str().unwrap().to_string(), f, Some(c))
            })
            .collect();
        let mut pending = fixture(&json!({"modules":{"a":"let x=1;","b":""}}), 2);
        pending.files.insert(
            "input/b.toy".into(),
            format!("{}let x=1;{}", "{".repeat(24), "}".repeat(24)).into_bytes(),
        );
        pending
            .file_reads
            .insert("input/a.toy".into(), FileReadScript::Pending);
        if group == 0 {
            selected.push(("pending-cancel".into(), pending, None));
        }
        if remaining {
            for extra in extras() {
                let mut world = fixture(&extra, 2);
                if let Some(commands) = extra.get("commands") {
                    world.args = vec![
                        "session".into(),
                        "modules.json".into(),
                        "2".into(),
                        "session.json".into(),
                    ];
                    world.files.insert(
                        "input/session.json".into(),
                        json!({"commands":commands}).to_string().into_bytes(),
                    );
                } else {
                    world.args.push("cache.json".into());
                    if !extra["cache"].is_null() {
                        let data = if extra["cache"].is_string() {
                            extra["cache"].as_str().unwrap().as_bytes().to_vec()
                        } else {
                            extra["cache"].to_string().into_bytes()
                        };
                        world.files.insert("output/cache.json".into(), data);
                    }
                }
                selected.push((extra["id"].as_str().unwrap().into(), world, Some(extra)));
            }
        }
        if readiness {
            selected = ready_worlds();
        }
        if missing {
            selected = missing_worlds();
        }
        if widths {
            selected = ready_worlds()
                .into_iter()
                .filter(|(id, _, _)| id == "ready-release-j2" || id == "ready-denied")
                .collect();
        }
        assert!(!selected.is_empty());
        for (id, world, case) in selected {
            let steps = case
                .as_ref()
                .and_then(|c| c["steps"].as_u64())
                .unwrap_or(1_000_000);
            let expected =
                if remaining && case.as_ref().is_some_and(|c| c.get("expected").is_some()) {
                    // These reference/checked cells already passed in the original oracle test.
                    // Native expected bytes come from the independently frozen vector instead
                    // of executing unchanged reference cells again for harness convenience.
                    expected_case(case.as_ref().unwrap())
                } else {
                    let reference =
                        run_case(&mut loaded, &world, &format!("native-oracle-{id}"), steps);
                    if let Some(case) = &case {
                        if let Some(expected_rows) = case.get("expected_rows") {
                            assert_eq!(
                                rows(&reference.output),
                                expected_rows.as_array().unwrap().to_vec(),
                                "{id}"
                            );
                            assert_eq!(
                                reference.exit_code,
                                Some(case["exit"].as_i64().unwrap() as i32)
                            );
                            assert!(reference.failures.is_empty() && reference.blocked.is_none());
                        } else if case.get("expected").is_some() {
                            assert_case(case, &reference);
                        } else if case["id"].as_str().unwrap().starts_with("ready-") {
                            assert_ready(case, &reference);
                        } else {
                            assert_extra(case, &reference);
                        }
                    }
                    Observation::of_reference(&reference)
                };
            let mut runnables = vec![];
            for (backend, objects) in &objects {
                let placements: &[u64] = if widths { &[1, 2, 4] } else { &[0] };
                for width in placements {
                    let dir = scratch.path().join(format!("{id}-{backend:?}-w{width}"));
                    fs::create_dir_all(&dir).unwrap();
                    eprintln!("TOY_STARTED {id} {backend:?} requested_width={width}");
                    let runnable =
                        link_fixture_runnable(&ready, &loaded.entry, objects, &tools, &dir, &world)
                            .unwrap();
                    if *width != 0 {
                        let run_file = dir.join("run.json");
                        let mut run: Value =
                            serde_json::from_slice(&fs::read(&run_file).unwrap()).unwrap();
                        run["placement"] = json!({"cores":width,"commit":"shared","reorder":"off"});
                        fs::write(run_file, run.to_string()).unwrap();
                    }
                    runnables.push((runnable, dir));
                }
            }
            let mut streams: BTreeMap<String, hirsel_rt::launch::Recording> = BTreeMap::new();
            for (runnable, dir) in &runnables {
                let tape = dir.join("recording.tape");
                let recording = launch_fixture(
                    runnable,
                    &ready.target,
                    &tools,
                    dir,
                    true,
                    Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                    &world,
                )
                .unwrap();
                assert_eq!(
                    observe(&recording, &ready, &sites).unwrap(),
                    expected,
                    "{id}"
                );
                let bytes = fs::read(&tape).unwrap();
                let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                if widths {
                    let mut semantic = record.clone();
                    if let Some(binding) = semantic.binding.as_mut() {
                        binding.placement = None;
                        binding.admitted_at = 0;
                    }
                    let key = dir
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .rsplit_once("-w")
                        .unwrap()
                        .0
                        .to_string();
                    if let Some(previous) = streams.get(&key) {
                        assert_eq!(previous, &semantic, "exact scripted semantic stream: {key}");
                    } else {
                        streams.insert(key, semantic);
                    }
                    eprintln!(
                        "TOY_SCRIPTED_WIDTH_STREAM_PASS {} requested width only; no physical-capacity claim",
                        dir.display()
                    );
                }
                for (part, chunk) in bytes.chunks(4096).enumerate() {
                    let hex: String = chunk.iter().map(|byte| format!("{byte:02x}")).collect();
                    eprintln!(
                        "TOY_TAPE {id} {} {part} {hex}",
                        dir.file_name().unwrap().to_string_lossy()
                    );
                }
                if id == "pending-cancel" || id.starts_with("ready-release") || id == "ready-denied"
                {
                    assert!(record.events.iter().any(|e| matches!(
                        (&e.request, &e.completion),
                        (
                            hirsel_rt::io::Request::FileRead { .. },
                            hirsel_rt::io::Completion::Blocked { committed: false }
                        )
                    )));
                    assert!(
                        record
                            .events
                            .iter()
                            .any(|e| matches!(e.request, hirsel_rt::io::Request::Close { .. }))
                    );
                }
                let absent = Fixture {
                    files: BTreeMap::new(),
                    file_reads: BTreeMap::new(),
                    ..world.clone()
                };
                for (target, replay_dir) in &runnables {
                    let replay = launch_fixture(
                        target,
                        &ready.target,
                        &tools,
                        replay_dir,
                        true,
                        Tape::Replay(&tape),
                        &absent,
                    )
                    .unwrap();
                    assert_eq!(observe(&replay, &ready, &sites).unwrap(), expected);
                }
                for checked in [false, true] {
                    let replay = |tape: &Path| {
                        let mut program = compiler.load_program(&path)?;
                        hirsel_driver::replay_loaded_observations(
                            &mut program,
                            tape,
                            Some(steps),
                            checked,
                        )
                    };
                    let observed = replay(&tape).unwrap();
                    assert_eq!(Observation::of_reference(&observed), expected);
                    let mut changed = record.clone();
                    changed.events.push(record.events[0].clone());
                    let bad = dir.join("extra.tape");
                    fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap())
                        .unwrap();
                    assert!(replay(&bad).is_err());
                }
                assert_eq!(fs::read(&tape).unwrap(), bytes);
                eprintln!("TOY_PASS {id} {} and four replays", dir.display());
            }
        }
        if std::env::var_os("TOY_ANALYZER_31_EVIDENCE").is_some() {
            let _ = scratch.keep();
        }
    });
}
#[test]
#[ignore = "physical width qualification: run this identity with hirsel.tests=host; pool capacity can be lower than four"]
fn toy_analyzer_31_production_ordered_real_files() {
    let scratch = scratch();
    let path = package(scratch.path());
    fs::write(
        scratch.path().join("production.py"),
        include_str!("../support/toy_analyzer_31/production.py"),
    )
    .unwrap();
    fs::write(
        scratch.path().join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(scratch.path().join("production.py"))
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg(&path)
        .output()
        .unwrap();
    fs::write(scratch.path().join("production.stdout"), &output.stdout).unwrap();
    fs::write(scratch.path().join("production.stderr"), &output.stderr).unwrap();
    let root = scratch.keep();
    assert!(
        output.status.success(),
        "{}: {} {}",
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "offline audit of existing immutable host tapes; set TOY_ANALYZER_31_ARCHIVES and use hirsel.tests=host"]
fn toy_analyzer_31_host_tapes_checked_replay_and_controls() {
    hirsel_engine::with_compiler_stack(|| {
        let directories = std::env::var("TOY_ANALYZER_31_ARCHIVES")
            .expect("explicit archived launch directories");
        let tool_seal = scratch();
        let _seal_manifest = package(tool_seal.path());

        let mut count = 0;
        for directory in directories.split(',').map(PathBuf::from) {
            let path = directory.join("hirsel.json");
            let mut program = hirsel_driver::load_program(&path).unwrap();
            let mut tapes: Vec<_> = fs::read_dir(&directory)
                .unwrap()
                .map(|p| p.unwrap().path())
                .filter(|p| p.extension().is_some_and(|s| s == "tape"))
                .collect();
            tapes.sort();
            let mut normalized: BTreeMap<String, hirsel_rt::launch::Recording> = BTreeMap::new();
            let source_control = scratch();
            let changed_path = package(source_control.path());
            // A semantic change (not a comment) must refuse the original tape binding.
            let source = fs::read_to_string(source_control.path().join("report.hir")).unwrap();
            fs::write(
                source_control.path().join("report.hir"),
                source.replace("(\"schema\", number(1))", "(\"schema\", number(2))"),
            )
            .unwrap();
            let mut source_program = hirsel_driver::load_program(&changed_path).unwrap();
            let mut source_control_done = false;
            for tape in tapes {
                let name = tape.file_stem().unwrap().to_str().unwrap();
                if name.starts_with("tampered") {
                    continue;
                }
                if let Ok(selected) = std::env::var("TOY_ANALYZER_31_ARCHIVE_CELLS") {
                    if !selected.split(',').any(|cell| cell == name) {
                        continue;
                    }
                }

                let original = fs::read(&tape).unwrap();
                let record = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
                if !source_control_done {
                    for checked in [false, true] {
                        assert!(
                            hirsel_driver::replay_loaded_observations(
                                &mut source_program,
                                &tape,
                                Some(1_000_000),
                                checked,
                            )
                            .is_err()
                        );
                    }
                    source_control_done = true;
                }

                let expected = fs::read(tape.with_extension("stdout")).unwrap();
                let code = if name.contains("cancel") {
                    2
                } else if name.contains("domain") {
                    1
                } else {
                    0
                };
                assert!(!directory.join("input/a.toy").exists());
                assert!(!directory.join("input/b.toy").exists());
                let a = hirsel_driver::replay_loaded_observations(
                    &mut program,
                    &tape,
                    Some(1_000_000),
                    false,
                )
                .unwrap();
                let b = hirsel_driver::replay_loaded_observations(
                    &mut program,
                    &tape,
                    Some(1_000_000),
                    true,
                )
                .unwrap();
                assert_eq!(a, b, "{name}: exact checked replay including event order");
                assert_eq!(a.output, expected);
                assert_eq!(a.exit_code, Some(code));
                assert!(a.failures.is_empty() && a.blocked.is_none());
                let mut semantic = record.clone();
                let binding = semantic.binding.as_mut().unwrap();
                binding.admitted_at = 0;
                binding.placement = None;
                // Compare exact file/capability event streams across widths, preserving IDs,
                // all payloads and completions. Placement/admission are the only normalization.
                let group = name.rsplit_once("-w").unwrap().0.to_string();
                if let Some(prior) = normalized.get(&group) {
                    assert_eq!(prior, &semantic, "{name}");
                } else {
                    normalized.insert(group, semantic);
                }
                if name.contains("cancel") {
                    let handle = record
                        .events
                        .iter()
                        .find_map(|e| match (&e.request, &e.completion) {
                            (
                                hirsel_rt::io::Request::Open { path, .. },
                                hirsel_rt::io::Completion::Handle(Ok(handle)),
                            )
                            | (
                                hirsel_rt::io::Request::Open { path, .. },
                                hirsel_rt::io::Completion::FileInput(
                                    hirsel_rt::io::FileInput::Opened(handle),
                                ),
                            ) if path == "a.toy" => Some(*handle),
                            _ => None,
                        })
                        .expect("canceled source was opened");
                    let reads: usize = record
                        .events
                        .iter()
                        .filter_map(|e| match (&e.request, &e.completion) {
                            (
                                hirsel_rt::io::Request::FileRead { file, .. },
                                hirsel_rt::io::Completion::FileInput(
                                    hirsel_rt::io::FileInput::Bytes(bytes),
                                ),
                            ) if *file == handle => Some(bytes.len()),
                            _ => None,
                        })
                        .sum();
                    assert!(
                        reads < 8192,
                        "younger refusal did not suppress remaining reads"
                    );
                    assert!(record.events.iter().any(|e|matches!(e.request,hirsel_rt::io::Request::Close{handle:h} if h==handle)),"canceled source was not closed");
                    assert!(a.effects.iter().any(|e|matches!(&e.effect,hirsel_interp::observation::Effect::CapabilityConsumed(c) if c.operation=="task.cancel_all")),"cancellation was not observed");
                }
                for checked in [false, true] {
                    let mut replay = |tape: &Path| {
                        hirsel_driver::replay_loaded_observations(
                            &mut program,
                            tape,
                            Some(1_000_000),
                            checked,
                        )
                    };
                    let mut extra = record.clone();
                    extra.events.push(record.events[0].clone());
                    let bad = directory.join(format!("tampered-{name}.tape"));
                    fs::write(&bad, hirsel_rt::launch::render_recording(&extra).unwrap()).unwrap();
                    assert!(
                        replay(&bad)
                            .unwrap_err()
                            .to_string()
                            .contains("UnconsumedInput")
                    );
                    let mut changed = record.clone();
                    if let hirsel_rt::io::Request::Stdout { io } = &mut changed.events[0].request {
                        *io += 99;
                    }
                    fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap())
                        .unwrap();
                    assert!(replay(&bad).is_err());
                    let mut changed_config = record.clone();
                    changed_config.arguments.push("changed-config".into());
                    fs::write(
                        &bad,
                        hirsel_rt::launch::render_recording(&changed_config).unwrap(),
                    )
                    .unwrap();
                    assert!(replay(&bad).is_err());
                }
                // Cross-backend replay with real providers absent. Native own-backend replays
                // were completed by the launch script and are not repeated here.
                if name.starts_with("llvm") {
                    let backend = "cranelift";
                    let launched = Command::new("python3")
                        .arg(directory.join("capped"))
                        .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
                        .arg(directory.join(backend))
                        .arg("--hirsel-replay")
                        .arg(&tape)
                        .env("HIRSEL_JOBS", "4")
                        .env("HIRSEL_MEMORY_LIMIT", "2G")
                        .output()
                        .unwrap();
                    assert_eq!(launched.status.code(), Some(code), "{name}: {launched:?}");
                    assert_eq!(launched.stdout, expected);
                }
                assert_eq!(fs::read(&tape).unwrap(), original);
                assert!(!directory.join("input/a.toy").exists());
                count += 1;
                eprintln!(
                    "TOY_ARCHIVE_PASS {name} reference checked-h cross-native complete-consumption immutable"
                );
            }
        }
        assert!(count > 0);
        eprintln!("TOY_ARCHIVE_COMPLETED {count}");
    });
}

fn extras() -> Vec<Value> {
    serde_json::from_str(include_str!("../support/toy_analyzer_31/extras.json")).unwrap()
}
fn assert_extra(case: &Value, observed: &hirsel_interp::observation::Observable) {
    let id = case["id"].as_str().unwrap();
    assert!(
        observed.failures.is_empty() && observed.blocked.is_none(),
        "{id}: {observed:?}"
    );
    let output = rows(&observed.output);
    let last = output.last().unwrap();
    if id == "edits33" {
        assert_eq!(last, &json!({"error":"edits"}));
        assert_eq!(observed.exit_code, Some(2));
    } else if id == "edits32" {
        assert_eq!(last["reused"], json!(["a"]));
        assert_eq!(last["analyzed"], json!([]));
        assert_eq!(observed.exit_code, Some(0));
    } else if id == "cache-corrupt" {
        assert_eq!(last, &json!({"error":"json"}));
        assert_eq!(observed.exit_code, Some(2));
    } else if id == "cache-version" {
        assert_eq!(last, &json!({"error":"cache"}));
        assert_eq!(observed.exit_code, Some(2));
    } else {
        assert_eq!(observed.exit_code, Some(0));
        assert_eq!(last["total"], 0);
        let cache = observed
            .final_state
            .as_ref()
            .unwrap()
            .trees
            .values()
            .find_map(|(_, files)| files.get("output/cache.json"))
            .expect("published cache");
        let cache: Value = serde_json::from_slice(cache).unwrap();
        assert_eq!(cache["schema"], 1);
        assert_eq!(
            cache["modules"][0]["exports"],
            json!([{"name":"n","type":"int"}])
        );
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(b"let n=1;"));
        assert_eq!(cache["modules"][0]["fingerprint"], hash);
    }
}

fn expected_case(case: &Value) -> Observation {
    let rows = case["expected"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            format!(
                "{{\"module\":{},\"code\":{},\"start\":{},\"end\":{}}}",
                d["module"], d["code"], d["start"], d["end"]
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let names: Vec<_> = case["modules"].as_object().unwrap().keys().collect();
    let output = format!(
        "{{\"diagnostics\":[{}],\"total\":{},\"analyzed\":{},\"reused\":[]}}\n",
        rows,
        case["expected"]["total"],
        serde_json::to_string(&names).unwrap()
    )
    .into_bytes();
    Observation {
        exit_code: Some(case["exit"].as_i64().unwrap() as i32),
        blocked: false,
        budget: None,
        signaled: None,
        output,
        failures: vec![],
        roots: vec![],
    }
}

#[test]
fn toy_analyzer_31_ready_release_affected() {
    scripted_native(2)
}
fn ready_worlds() -> Vec<(String, Fixture, Option<Value>)> {
    let mut selected = vec![];
    for (id, jobs, marker) in [
        ("ready-release-j1", 1, json!("ready")),
        ("ready-release-j2", 2, json!("ready")),
        ("ready-release-j4", 4, json!("ready")),
        ("ready-default", 2, Value::Null),
        ("ready-denied", 2, json!("ready")),
        ("ready-invalid", 2, json!("../ready")),
        ("ready-shape", 2, json!(false)),
    ] {
        let mut world = fixture(&json!({"modules":{"a":"let n=1;"}}), jobs);
        if !marker.is_null() {
            let mut manifest: Value =
                serde_json::from_slice(&world.files["input/modules.json"]).unwrap();
            manifest["ready"] = marker;
            world.files.insert(
                "input/modules.json".into(),
                manifest.to_string().into_bytes(),
            );
        }
        if id.starts_with("ready-release") {
            world.file_reads.insert(
                "input/a.toy".into(),
                FileReadScript::AfterWrite {
                    path: "output/ready".into(),
                },
            );
        }
        if id == "ready-denied" {
            world.mounts.insert("output".into(), Access::Read);
            world
                .file_reads
                .insert("input/a.toy".into(), FileReadScript::Pending);
        }
        selected.push((id.into(), world, Some(json!({"id":id,"steps":1000000}))));
    }
    selected
}
fn assert_ready(case: &Value, observed: &hirsel_interp::observation::Observable) {
    let id = case["id"].as_str().unwrap();
    let output = rows(&observed.output);
    let error = match id {
        "ready-denied" => Some("file"),
        "ready-invalid" => Some("path"),
        "ready-shape" => Some("shape"),
        _ => None,
    };
    if let Some(error) = error {
        assert_eq!(
            output,
            json!([{"error":error}]).as_array().unwrap().to_vec()
        );
        assert_eq!(observed.exit_code, Some(2));
    } else {
        assert_eq!(
            output,
            json!([{"diagnostics":[],"total":0,"analyzed":["a"],"reused":[]}])
                .as_array()
                .unwrap()
                .to_vec()
        );
        assert_eq!(observed.exit_code, Some(0));
    }
    if id.starts_with("ready-release") || id == "ready-denied" {
        assert!(observed.effects.iter().any(|e|matches!(&e.effect,hirsel_interp::observation::Effect::CapabilityConsumed(c) if c.operation=="file.read.pending")),"{id}: no actual parked read");
    }
    let files = &observed
        .final_state
        .as_ref()
        .unwrap()
        .trees
        .values()
        .next()
        .unwrap()
        .1;
    if id.starts_with("ready-release") {
        assert_eq!(files.get("output/ready").unwrap(), b"ready");
    } else {
        assert!(!files.contains_key("output/ready"));
    }
}

#[test]
fn toy_analyzer_31_incremental_native_and_missing_vectors() {
    scripted_native(3)
}
fn missing_worlds() -> Vec<(String, Fixture, Option<Value>)> {
    let session: Value = serde_json::from_str(include_str!(
        "../support/toy_analyzer_31/native-session.json"
    ))
    .unwrap();
    let mut world = fixture(&session, 2);
    world.args = vec![
        "session".into(),
        "modules.json".into(),
        "2".into(),
        "session.json".into(),
    ];
    world.files.insert(
        "input/session.json".into(),
        json!({"commands":session["commands"]})
            .to_string()
            .into_bytes(),
    );
    let mut selected = vec![("incremental-native-session".into(), world, Some(session))];
    let mut denied = fixture(&json!({"modules":{"a":"let n=1;"}}), 2);
    denied.mounts.remove("input");
    selected.push(("input-denied-native".into(),denied,Some(json!({"id":"input-denied-native","exit":2,"steps":1000000,"expected_rows":[{"error":"file"}]}))));
    let recovery = json!({"id":"parser-recovery","modules":{"a":"let = ; let x=y;"},"steps":1000000,"exit":1,"expected_rows":[{"diagnostics":[{"module":"a","code":"syntax","start":4,"end":5},{"module":"a","code":"syntax","start":4,"end":5},{"module":"a","code":"undefined","start":14,"end":15}],"total":3,"analyzed":["a"],"reused":[]}]});
    selected.push((
        "parser-recovery".into(),
        fixture(&recovery, 2),
        Some(recovery),
    ));
    let unknown = json!({"id":"unknown-import","modules":{"a":"import missing; let n=1;"},"steps":1000000,"exit":1,"expected_rows":[{"diagnostics":[{"module":"a","code":"unknown_import","start":7,"end":14}],"total":1,"analyzed":["a"],"reused":[]}]});
    selected.push(("unknown-import".into(), fixture(&unknown, 2), Some(unknown)));
    // Exact retained diagnostic boundary complements the existing 129 vector.
    let mut cap = cases()
        .into_iter()
        .find(|c| c["id"] == "diagnostics129")
        .unwrap();
    cap["id"] = "diagnostics128".into();
    let source = (0..128)
        .map(|i| format!("let x{i}=y;"))
        .collect::<Vec<_>>()
        .join(" ");
    cap["modules"]["a"] = source.into();
    cap["expected"]["total"] = 128.into();
    selected.push(("diagnostics128".into(), fixture(&cap, 2), Some(cap)));
    selected
}

#[test]
fn toy_analyzer_31_scripted_width_streams() {
    scripted_native(4)
}

#[test]
#[ignore = "negative-only existing tape audit; explicit host archive, no live recording or positive replay"]
fn toy_analyzer_31_exact_refusal_controls() {
    hirsel_engine::with_compiler_stack(|| {
        let directory = PathBuf::from(std::env::var("TOY_ANALYZER_31_REFUSAL_ARCHIVE").unwrap());
        let manifest = directory.join("hirsel.json");
        let tape = directory.join("cranelift-success-w1.tape");
        let bytes = fs::read(&tape).unwrap();
        let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
        let scratch = scratch();
        let changed = package(scratch.path());
        let report = scratch.path().join("report.hir");
        let source = fs::read_to_string(&report).unwrap();
        let replacement = source.replacen("number(1)", "number(2)", 1);
        assert_ne!(source, replacement);
        fs::write(&report, replacement).unwrap();
        for replay in [
            hirsel_driver::replay_observations,
            hirsel_driver::replay_observations_checked,
        ] {
            let error = replay(&changed, &tape, Some(1_000_000))
                .unwrap_err()
                .to_string();
            eprintln!("TOY_REFUSED changed-source {error}");
            assert!(error.contains("program") || error.contains("Program"));
            assert!(!error.contains("ToolingLimit"));
            let mut input = record.clone();
            input.arguments.push("changed-config".into());
            let bad = scratch.path().join("input.tape");
            fs::write(&bad, hirsel_rt::launch::render_recording(&input).unwrap()).unwrap();
            let error = replay(&manifest, &bad, Some(1_000_000))
                .unwrap_err()
                .to_string();
            eprintln!("TOY_REFUSED changed-config {error}");
            assert!(error.contains("header disagrees with its run record"));
            assert!(!error.contains("ToolingLimit"));
            let mut extra = record.clone();
            extra.events.push(record.events[0].clone());
            fs::write(&bad, hirsel_rt::launch::render_recording(&extra).unwrap()).unwrap();
            let error = replay(&manifest, &bad, Some(1_000_000))
                .unwrap_err()
                .to_string();
            eprintln!("TOY_REFUSED unconsumed {error}");
            assert!(error.contains("UnconsumedInput"));
            let mut event = record.clone();
            if let hirsel_rt::io::Request::Stdout { io } = &mut event.events[0].request {
                *io += 99;
            } else {
                panic!("expected stdout projection");
            }
            fs::write(&bad, hirsel_rt::launch::render_recording(&event).unwrap()).unwrap();
            let error = replay(&manifest, &bad, Some(1_000_000))
                .unwrap_err()
                .to_string();
            eprintln!("TOY_REFUSED changed-event {error}");
            assert!(error.contains("ReplayMismatch"));
            assert!(!error.contains("ToolingLimit"));
        }
        assert_eq!(fs::read(&tape).unwrap(), bytes);
    });
}

#[test]
#[ignore = "structured offline first difference of immutable recordings"]
fn toy_analyzer_31_recording_difference() {
    let directory = PathBuf::from(std::env::var("TOY_ANALYZER_31_REFUSAL_ARCHIVE").unwrap());
    let mut a = hirsel_rt::host_launch::read_recording(&directory.join("cranelift-domain-w1.tape"))
        .unwrap();
    let mut b = hirsel_rt::host_launch::read_recording(&directory.join("cranelift-domain-w2.tape"))
        .unwrap();
    for record in [&mut a, &mut b] {
        let binding = record.binding.as_mut().unwrap();
        binding.admitted_at = 0;
        binding.placement = None;
    }
    assert_eq!(a.arguments, b.arguments);
    assert_eq!(a.run_identity, b.run_identity);
    assert_eq!(a.tree, b.tree);
    assert_eq!(a.binding, b.binding);
    let (index, (left, right)) = a
        .events
        .iter()
        .zip(&b.events)
        .enumerate()
        .find(|(_, (a, b))| a != b)
        .expect("retained original red");
    eprintln!(
        "TOY_FIRST_DIFFERENCE header=equal event_index={index} left={left:?} right={right:?}"
    );
    assert_eq!(left.request, right.request);
    match (&left.completion, &right.completion) {
        (
            hirsel_rt::io::Completion::Metadata(Ok(a)),
            hirsel_rt::io::Completion::Metadata(Ok(b)),
        ) => {
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.size, b.size);
            assert_eq!(a.permissions, b.permissions);
            assert_ne!(a.modified_unix_ns, b.modified_unix_ns);
        }
        _ => panic!("unexpected first difference"),
    }
}
