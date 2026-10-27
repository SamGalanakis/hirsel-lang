//! Full six-module entry workflows; expected edits use an independent full oracle.
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
};

fn package(root: &Path) -> PathBuf {
    for (name, text) in [
        ("main", include_str!("../../../../examples/sheet/main.hir")),
        (
            "formula",
            include_str!("../../../../examples/sheet/formula.hir"),
        ),
        (
            "cells",
            include_str!("../../../../examples/sheet/cells.hir"),
        ),
        (
            "graph",
            include_str!("../../../../examples/sheet/graph.hir"),
        ),
        (
            "evaluate",
            include_str!("../../../../examples/sheet/evaluate.hir"),
        ),
        (
            "store",
            include_str!("../../../../examples/sheet/store.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), text).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/sheet/hirsel.json")).unwrap();
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
fn scratch(label: &str) -> tempfile::TempDir {
    let base = std::env::var_os("SHEET_EVAL_31_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
}
fn cases() -> Vec<Value> {
    let mut cases: Vec<Value> =
        serde_json::from_str(include_str!("../support/sheet_eval_31/cases.json")).unwrap();
    if let Ok(filter) = std::env::var("SHEET_EVAL_31_CASE") {
        cases.retain(|c| filter.split(',').any(|id| c["id"] == id));
    }
    assert!(!cases.is_empty(), "zero case selection");
    cases
}
fn fixture(case: &Value) -> Fixture {
    let mut files = BTreeMap::from([(
        "work/script".into(),
        case["script"].as_str().unwrap().as_bytes().to_vec(),
    )]);
    if let Some(extra) = case["files"].as_object() {
        for (name, content) in extra {
            files.insert(
                format!("work/{name}"),
                content.as_str().unwrap().as_bytes().to_vec(),
            );
        }
    }
    let mut recipe = Fixture {
        args: vec!["--jobs".into(), "4".into(), "script".into()],
        files,
        mounts: BTreeMap::from([("work".into(), Access::ReadWrite)]),
        steps: Some(case["steps"].as_u64().unwrap()),
        ..Default::default()
    };
    if let Some(stdin) = case["stdin"].as_str() {
        recipe.stdin = stdin.as_bytes().to_vec();
    }
    if let Some(args) = case["args"].as_array() {
        recipe.args = args
            .iter()
            .map(|arg| arg.as_str().unwrap().to_owned())
            .collect();
    }
    if case["cancel"] == true {
        recipe.args = vec!["--interruptible".into(), "script".into()];
        recipe
            .file_reads
            .insert("work/script".into(), FileReadScript::Pending);
        recipe.signals = Some(
            serde_json::from_value(
                json!({"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]}),
            )
            .unwrap(),
        );
    }
    if case["release"] == true {
        recipe.file_reads.insert(
            "work/script".into(),
            FileReadScript::AfterWrite {
                path: "work/release".into(),
            },
        );
        recipe.signals = Some(
            serde_json::from_value(
                json!({"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]}),
            )
            .unwrap(),
        );
    }
    if case["deny_output"] == true {
        recipe.deny_output = true;
    }
    recipe
}

#[test]
fn sheet_eval_31_scripted_reference_checked_entry() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch("sheet-scripted-");
        let path = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let selected = cases();
        eprintln!("SHEET_SELECTED {}", selected.len() * 2);
        for case in selected {
            let fixture = fixture(&case);
            let budget = fixture.steps.unwrap();
            let mut previous = None;
            for checked in [false, true] {
                let id = case["id"].as_str().unwrap();
                eprintln!("SHEET_STARTED {id} checked={checked} budget={budget}");
                let execution = if checked {
                    run_observed_checked_bounded(
                        &mut loaded.engine,
                        &run,
                        (&fixture).into(),
                        budget,
                    )
                } else {
                    run_observed_snapshot_bounded(
                        &mut loaded.engine,
                        &run,
                        (&fixture).into(),
                        budget,
                    )
                }
                .unwrap();
                eprintln!(
                    "SHEET_USAGE {id} checked={checked} {:?}",
                    execution.step_usage
                );
                let observed = execution
                    .observation
                    .as_ref()
                    .unwrap_or_else(|| panic!("INCOMPLETE {id}/{checked}: {execution:?}"));
                assert_eq!(
                    observed.output,
                    case["stdout"].as_str().unwrap().as_bytes(),
                    "{id}/{checked}"
                );
                assert_eq!(
                    observed.exit_code,
                    Some(case["exit"].as_i64().unwrap() as i32),
                    "{id}/{checked}: {observed:?}"
                );
                assert!(
                    observed.failures.is_empty() && observed.blocked.is_none(),
                    "{id}/{checked}: {observed:?}"
                );
                if let Some(before) = previous {
                    assert_eq!(*observed, before, "{id} full event order");
                }
                previous = Some(observed.clone());
                eprintln!("SHEET_PASS {id} checked={checked}");
            }
        }
        let _ = scratch.keep();
    });
}

fn native_cases(selected: Vec<Value>) {
    hirsel_engine::with_compiler_stack(|| {
        let retained = scratch("sheet-native-").keep();
        let scratch = retained.as_path();
        let path = package(scratch);
        if selected
            .iter()
            .any(|case| case["release"] == true || case["recalc_cancel"] == true)
        {
            fs::write(
                scratch.join("release.hir"),
                if selected.iter().any(|case| case["release"] == true) {
                    include_str!("../support/sheet_eval_31/release.hir")
                } else {
                    include_str!("../support/sheet_eval_31/cancel_recalc.hir")
                },
            )
            .unwrap();
            let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            manifest["sources"][0]["module"] = json!(["app"]);
            manifest["sources"]
                .as_array_mut()
                .unwrap()
                .push(json!({"path":"release.hir","module":[]}));
            fs::write(&path, manifest.to_string()).unwrap();
        }
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        if selected.iter().any(|case| {
            case["release"] == true || case["recalc_cancel"] == true || case["stdin"].is_string()
        }) {
            let run = RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            for case in &selected {
                let f = fixture(case);
                let mut prior = None;
                for checked in [false, true] {
                    let execution = if checked {
                        run_observed_checked_bounded(
                            &mut loaded.engine,
                            &run,
                            (&f).into(),
                            1_000_000,
                        )
                    } else {
                        run_observed_snapshot_bounded(
                            &mut loaded.engine,
                            &run,
                            (&f).into(),
                            1_000_000,
                        )
                    }
                    .unwrap();
                    let o = execution.observation.expect("release observation");
                    assert_eq!(o.output, case["stdout"].as_str().unwrap().as_bytes());
                    assert_eq!(o.exit_code, Some(case["exit"].as_i64().unwrap() as i32));
                    assert!(o.failures.is_empty() && o.blocked.is_none());
                    if case["release"] == true {
                        assert!(o.effects.iter().any(|e|matches!(&e.effect,hirsel_interp::observation::Effect::CapabilityConsumed(e) if e.operation=="file.read.pending")),"release read never parked");
                    }
                    if let Some(before) = prior {
                        assert_eq!(o, before);
                    }
                    prior = Some(o);
                    eprintln!("SHEET_SUPPLEMENTAL_PASS {} checked={checked}", case["id"]);
                }
            }
        }
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let ready = build(&validated, &options).unwrap();
        let sites = Sites::new(validated.snapshot());
        let mut tools = Toolchain::in_tree(&scratch.join("runtime"), &[&ready.target]).unwrap();
        tools.launch_timeout = std::time::Duration::from_secs(70);
        tools.execution = Execution::Executable;
        fs::write(
            scratch.join("capped"),
            include_str!("../../../../tools/capped"),
        )
        .unwrap();
        fs::write(
            scratch.join("launch.py"),
            include_str!("../support/sheet_eval_31/launch.py"),
        )
        .unwrap();
        tools.wrapper = vec![
            "/usr/bin/python3".into(),
            scratch.join("launch.py").to_string_lossy().into_owned(),
        ];
        let mut artifacts = vec![];
        for backend in [Backend::Llvm, Backend::Cranelift] {
            eprintln!("SHEET_BUILD {backend:?}");
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            artifacts.push((backend, objects));
        }
        for case in selected {
            let id = case["id"].as_str().unwrap();
            let fixture = fixture(&case);
            let mut recorded = vec![];
            let mut semantic = None;
            for (backend, objects) in &artifacts {
                let widths = if case["widths"] == true {
                    vec![1, 2, 4]
                } else {
                    vec![1]
                };
                for width in widths {
                    let dir = scratch.join(format!("{id}-{backend:?}-w{width}"));
                    fs::create_dir(&dir).unwrap();
                    eprintln!("SHEET_STARTED {id} {backend:?}");
                    let runnable = link_fixture_runnable(
                        &ready,
                        &loaded.entry,
                        objects,
                        &tools,
                        &dir,
                        &fixture,
                    )
                    .unwrap();
                    if case["widths"] == true {
                        let run = dir.join("run.json");
                        let mut config: Value =
                            serde_json::from_slice(&fs::read(&run).unwrap()).unwrap();
                        config["placement"] =
                            json!({"cores":width,"commit":"shared","reorder":"auto"});
                        fs::write(run, config.to_string()).unwrap();
                    }
                    let tape = dir.join("recording");
                    let live = launch_fixture(
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
                    .unwrap();
                    if case["stdin"].is_string() {
                        let bytes = fs::read(&tape).unwrap();
                        eprintln!(
                            "SHEET_STDIN_TAPE {id} {backend:?} {}",
                            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
                        );
                    }
                    let observed = observe(&live, &ready, &sites).unwrap();
                    assert_eq!(
                        observed.output,
                        case["stdout"].as_str().unwrap().as_bytes(),
                        "{id}/{backend:?}"
                    );
                    assert_eq!(
                        observed.exit_code,
                        Some(case["exit"].as_i64().unwrap() as i32),
                        "{id}/{backend:?}"
                    );
                    assert!(
                        observed.failures.is_empty(),
                        "{id}/{backend:?}: {observed:?}"
                    );
                    if case["widths"] == true {
                        let mut rec =
                            hirsel_rt::launch::parse_recording_bytes(&fs::read(&tape).unwrap())
                                .unwrap();
                        let binding = rec.binding.as_mut().unwrap();
                        binding.placement = None;
                        binding.admitted_at = 0;
                        if let Some(before) = &semantic {
                            assert_eq!(
                                &rec, before,
                                "{id}/{backend:?}/w{width}: full semantic tape order"
                            );
                        } else {
                            semantic = Some(rec);
                        }
                        eprintln!("SHEET_ORDERED_TAPE_PASS {id}/{backend:?}/w{width}");
                    }
                    recorded.push((runnable, dir, tape, observed));
                    eprintln!("SHEET_PASS {id} {backend:?} w{width}");
                }
            }
            for (_, _, tape, original) in &recorded {
                let bytes = fs::read(tape).unwrap();
                if case["cancel"] == true || case["release"] == true {
                    let rec = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
                    assert!(
                        rec.events.iter().any(|e| matches!(
                            (&e.request, &e.completion),
                            (
                                hirsel_rt::io::Request::FileRead { .. },
                                hirsel_rt::io::Completion::Blocked { committed: false }
                            )
                        )),
                        "native read never parked"
                    );
                    assert!(
                        rec.events
                            .iter()
                            .any(|e| matches!(e.request, hirsel_rt::io::Request::Close { .. })),
                        "native file not closed"
                    );
                }

                if id.starts_with("edges-") {
                    // These exact host adapters already stopped at the unchanged 16M bound.
                    // Native-only cells never stand in for their missing observations.
                    eprintln!(
                        "SHEET_UNRUN_REPLAY {id} reference/checked-h: original16M Steps, see FRICTION.md"
                    );
                } else {
                    let ordinary = hirsel_driver::replay_loaded_observations(
                        &mut loaded,
                        tape,
                        fixture.steps,
                        false,
                    )
                    .unwrap();
                    let checked = hirsel_driver::replay_loaded_observations(
                        &mut loaded,
                        tape,
                        fixture.steps,
                        true,
                    )
                    .unwrap();
                    assert!(ordinary.equivalent(&checked));
                    assert_eq!(ordinary.output, original.output);
                    assert_eq!(ordinary.exit_code, original.exit_code);
                }
                // Replay has no original files or readiness gates.
                let unavailable = Fixture {
                    stdin: vec![],
                    files: BTreeMap::new(),
                    file_reads: BTreeMap::new(),
                    signals: None,
                    net: None,
                    process: None,
                    monotonic: None,
                    ..fixture.clone()
                };
                for (runnable, dir, _, _) in &recorded {
                    let replay = launch_fixture(
                        runnable,
                        &ready.target,
                        &tools,
                        dir,
                        true,
                        Tape::Replay(tape),
                        &unavailable,
                    )
                    .unwrap();
                    assert_eq!(
                        observe(&replay, &ready, &sites).unwrap(),
                        *original,
                        "{id} cross replay"
                    );
                }
                assert_eq!(fs::read(tape).unwrap(), bytes);
                eprintln!(
                    "SHEET_REPLAY_PASS {id} {} native=2 host={}",
                    tape.display(),
                    if id.starts_with("edges-") {
                        "INCOMPLETE Steps"
                    } else {
                        "2"
                    }
                );
            }
        }
    });
}

#[test]
fn sheet_eval_31_bounded_stdin_original_oracles() {
    // Original small REPL oracle (also used by repl_cancel_output_entry).
    let mut selected = vec![
        json!({"id":"bounded-small-session","args":["--jobs","4","-"],
        "script":"", "stdin":"set A1 = 2+3*4\nshow A1\n", "stdout":"OK\nA1 = 14\n", "exit":0,"steps":1_000_000}),
    ];
    // Admission counts the complete command before trimming, as the original
    // private reader did. The 513th byte refuses without executing the next line.
    for size in [512usize, 513] {
        let command = format!("recalc{}\nset A1 = 9\nshow A1\n", " ".repeat(size - 6));
        selected.push(json!({"id":format!("bounded-command-{size}"),"args":["-"],"script":"",
            "stdin":command,"stdout":if size==512 {"RECALC 0\nOK\nA1 = 9\n"} else {"error: line_limit\n"},
            "exit":if size==512 {0} else {3},"steps":1_000_000}));
    }
    native_cases(selected);
}

fn extra_cases() -> Vec<Value> {
    let mut items = vec![
        json!({"id":"invalid-range","script":"set A1 = SUM(B2:A1)\nshow A1\n","stdout":"OK\nA1 = #SYNTAX\n","exit":1,"steps":1_000_000}),
        json!({"id":"ast-cap","script":format!("set A1 = {}\nshow A1\n",vec!["1";16].join("+")),"stdout":"OK\nA1 = 16\n","exit":0,"steps":1_000_000}),
        json!({"id":"ast-over","script":format!("set A1 = {}\nshow A1\n",vec!["1";17].join("+")),"stdout":"error: depth_limit\nA1 = 0\n","exit":3,"steps":1_000_000}),
        json!({"id":"recalc-invalidation","script":"set A1 = 2\nset B1 = A1+1\nset P16 = 9\nrecalc\nset A1 = 4\nrecalc\nrecalc\nshow\n","stdout":"OK\nOK\nOK\nRECALC 3\nOK\nRECALC 2\nRECALC 0\nA1 = 4\nB1 = 5\nP16 = 9\n","exit":0,"steps":1_000_000}),
    ];
    let names: Vec<_> = (1..=16)
        .flat_map(|r| (b'A'..=b'P').map(move |c| format!("{}{r}", c as char)))
        .collect();
    for (id, overflow) in [("edges-4096", false), ("edges-4097", true)] {
        let rows:Vec<_>=names.iter().enumerate().map(|(i,n)|json!({"name":n,"formula":if overflow&&i==255 {"SUM(A1:P1)+A2"}else{"SUM(A1:P1)"}})).collect();
        items.push(json!({"id":id,"script":"set A1 = 9\nload big.json\nshow A1\n","files":{"big.json":json!({"version":1,"cells":rows}).to_string()},"stdout":if overflow {"OK\nerror: edge_limit\nA1 = 9\n"}else{"OK\nOK\nA1 = #CYCLE\n"},"exit":if overflow{3}else{1},"steps":16_000_000}));
    }
    for jobs in 1..=3 {
        items.push(json!({"id":format!("jobs-{jobs}"),"args":["--jobs",jobs.to_string(),"script"],"script":"set A1 = 2\nset B1 = 3\nset C1 = SUM(A1:B1)\nshow\n","stdout":"OK\nOK\nOK\nA1 = 2\nB1 = 3\nC1 = 5\n","exit":0,"steps":1_000_000}));
    }
    for n in [256usize, 257] {
        items.push(json!({"id":format!("unicode-{n}"),"script":"set A1 = 9\nload big.json\nshow A1\n","files":{"big.json":json!({"version":1,"cells":[{"name":"A1","formula":"é".repeat(n)}]}).to_string()},"stdout":if n==256{"OK\nOK\nA1 = #SYNTAX\n"}else{"OK\nerror: formula_limit\nA1 = 9\n"},"exit":if n==256{1}else{3},"steps":1_000_000}));
    }
    let mut rows: Vec<_> = names
        .iter()
        .map(|n| json!({"name":n,"formula":"1"}))
        .collect();
    rows.push(json!({"name":"A1","formula":"1"}));
    items.push(json!({"id":"cells-257","script":"load big.json\n","files":{"big.json":json!({"version":1,"cells":rows}).to_string()},"stdout":"error: cell_limit\n","exit":3,"steps":16_000_000}));
    items
}

#[test]
fn sheet_eval_31_additional_bounds_entry() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch("sheet-additional-");
        let path = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut incomplete = vec![];
        for case in extra_cases()
            .into_iter()
            .filter(|case| !case["id"].as_str().unwrap().starts_with("edges-"))
        {
            let f = fixture(&case);
            let id = case["id"].as_str().unwrap();
            for checked in [false, true] {
                eprintln!("SHEET_STARTED {id} checked={checked} budget={:?}", f.steps);
                let execution = if checked {
                    run_observed_checked_bounded(
                        &mut loaded.engine,
                        &run,
                        (&f).into(),
                        f.steps.unwrap(),
                    )
                } else {
                    run_observed_snapshot_bounded(
                        &mut loaded.engine,
                        &run,
                        (&f).into(),
                        f.steps.unwrap(),
                    )
                }
                .unwrap();
                eprintln!(
                    "SHEET_USAGE {id} checked={checked} {:?}",
                    execution.step_usage
                );
                if let Some(observed) = execution.observation {
                    assert_eq!(
                        observed.output,
                        case["stdout"].as_str().unwrap().as_bytes(),
                        "{id}/{checked}"
                    );
                    assert_eq!(
                        observed.exit_code,
                        Some(case["exit"].as_i64().unwrap() as i32)
                    );
                    assert!(observed.failures.is_empty() && observed.blocked.is_none());
                    eprintln!("SHEET_PASS {id} checked={checked}");
                } else {
                    incomplete.push(format!("{id}/{checked}: {:?}", execution.outcome));
                    eprintln!("SHEET_INCOMPLETE {}", incomplete.last().unwrap());
                }
            }
        }
        assert!(incomplete.is_empty(), "INCOMPLETE {incomplete:?}");
    });
}

#[test]
fn sheet_eval_31_repl_cancel_output_entry() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch("sheet-repl-");
        let path = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let base = fixture(&json!({"script":"show\n","steps":1_000_000}));
        for id in [
            "repl",
            "pending-cancel",
            "deny-output",
            "write-error",
            "jobs-0",
            "jobs-5",
            "load-refusal",
        ] {
            let mut f = base.clone();
            let (expected, status) = match id {
                "repl" => {
                    f.args = vec!["--jobs".into(), "4".into(), "-".into()];
                    (b"OK\nA1 = 14\n".as_slice(), 0)
                }
                "pending-cancel" => {
                    f.args = vec!["--interruptible".into(), "script".into()];
                    f.file_reads
                        .insert("work/script".into(), FileReadScript::Pending);
                    f.signals=Some(serde_json::from_value(json!({"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]})).unwrap());
                    (b"error: canceled\n".as_slice(), 3)
                }
                "deny-output" => {
                    f.deny_output = true;
                    (b"".as_slice(), 3)
                }
                "write-error" => {
                    f.files.insert("work/script".into(), b"recalc\n".to_vec());
                    (b"".as_slice(), 3)
                }
                "jobs-0" | "jobs-5" => {
                    f.args = vec![
                        "--jobs".into(),
                        if id == "jobs-0" { "0" } else { "5" }.into(),
                        "script".into(),
                    ];
                    (b"error: jobs_limit\n".as_slice(), 3)
                }
                _ => {
                    f.files.insert(
                        "work/script".into(),
                        b"set A1 = 9\nload missing\nshow A1\n".to_vec(),
                    );
                    (b"OK\nerror: file\nA1 = 9\n".as_slice(), 3)
                }
            };
            for checked in [false, true] {
                let mut world: hirsel_engine::SimulationConfig = (&f).into();
                if id == "repl" {
                    world.stdin = b"set A1 = 2+3*4\nshow A1\n".to_vec();
                }
                if id == "write-error" {
                    world.faults.insert(
                        "out.print".into(),
                        [hirsel_interp::library::simulation::Fault::Fault {
                            code: 7,
                            partial: 0,
                        }]
                        .into(),
                    );
                }
                eprintln!("SHEET_STARTED {id} checked={checked}");
                let execution = if checked {
                    run_observed_checked_bounded(&mut loaded.engine, &run, world, 1_000_000)
                } else {
                    run_observed_snapshot_bounded(&mut loaded.engine, &run, world, 1_000_000)
                }
                .unwrap();
                let o = execution
                    .observation
                    .unwrap_or_else(|| panic!("INCOMPLETE {id}: {:?}", execution.outcome));
                assert_eq!(o.output, expected, "{id}/{checked}");
                assert_eq!(o.exit_code, Some(status), "{id}/{checked}");
                assert!(o.failures.is_empty());
                if id == "pending-cancel" {
                    assert!(o.effects.iter().any(|e|matches!(&e.effect,hirsel_interp::observation::Effect::CapabilityConsumed(e) if e.operation=="file.read.pending")),"read never parked");
                    assert!(o.effects.iter().any(|e|matches!(&e.effect,hirsel_interp::observation::Effect::CapabilityConsumed(e) if e.operation=="file.close")),"file never closed");
                }
                eprintln!("SHEET_PASS {id} checked={checked}");
            }
        }
    });
}

#[test]
fn sheet_eval_31_native_full_entry_replay() {
    native_cases(cases());
}

#[test]
fn sheet_eval_31_native_additional_providers() {
    let mut selected = extra_cases();
    selected.push(json!({"id":"pending-cancel","script":"show\n","stdout":"error: canceled\n","exit":3,"steps":1_000_000,"cancel":true}));
    selected.push(json!({"id":"deny-output","script":"show\n","stdout":"","exit":3,"steps":1_000_000,"deny_output":true}));
    if let Ok(filter) = std::env::var("SHEET_EVAL_31_CASE") {
        selected.retain(|case| filter.split(',').any(|id| case["id"] == id));
    }
    assert!(!selected.is_empty(), "zero additional selection");
    native_cases(selected);
}

#[test]
#[ignore = "edges4096/4097 exhausted their original 16M reference/checked bound; INCOMPLETE, see FRICTION.md"]
fn sheet_eval_31_dense_reference_boundary() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch("sheet-dense-");
        let path = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for case in extra_cases()
            .into_iter()
            .filter(|case| case["id"].as_str().unwrap().starts_with("edges-"))
        {
            let f = fixture(&case);
            for checked in [false, true] {
                let execution = if checked {
                    run_observed_checked_bounded(&mut loaded.engine, &run, (&f).into(), 16_000_000)
                } else {
                    run_observed_snapshot_bounded(&mut loaded.engine, &run, (&f).into(), 16_000_000)
                }
                .unwrap();
                eprintln!("SHEET_DENSE_USAGE {:?}", execution.step_usage);
                let observed = execution
                    .observation
                    .expect("INCOMPLETE dense semantic observation");
                assert_eq!(observed.output, case["stdout"].as_str().unwrap().as_bytes());
                assert_eq!(
                    observed.exit_code,
                    Some(case["exit"].as_i64().unwrap() as i32)
                );
            }
        }
    });
}
#[test]
#[ignore = "requires SHEET_EVAL_31_PRODUCTION pointing at immutable production evidence; run explicitly on host"]
fn sheet_eval_31_production_host_replay() {
    hirsel_engine::with_compiler_stack(|| {
        let evidence = PathBuf::from(
            std::env::var_os("SHEET_EVAL_31_PRODUCTION").expect("production evidence root"),
        );
        let path = evidence.join("qualified-app/hirsel.json");
        let mut program = hirsel_driver::load_program(&path).unwrap();
        let fresh = std::env::var_os("SHEET_EVAL_31_FRESH_CANCEL").is_some();
        let mut rows = vec![];
        let mut incomplete = vec![];
        let mut directories: Vec<_> = fs::read_dir(evidence.join("production"))
            .unwrap()
            .map(|p| p.unwrap().path())
            .collect();
        directories.sort();
        for dir in directories {
            if !dir.join("PASS").exists() {
                continue;
            }
            // Historical runs selected only the previously unrun checked adapter.
            // Fresh identities after the repair exercise both public host adapters.
            let tape = dir.join("recording");
            let bytes = fs::read(&tape).unwrap();
            let expected = fs::read(dir.join("stdout")).unwrap();
            let status: i32 = fs::read_to_string(dir.join("exit"))
                .unwrap()
                .parse()
                .unwrap();
            let id = dir.file_name().unwrap().to_string_lossy();
            for checked in if fresh { vec![false, true] } else { vec![true] } {
                let engine = if checked { "checked-h" } else { "reference" };
                eprintln!("SHEET_STARTED production/{id}/{engine}");
                let attempt = hirsel_driver::replay_loaded_observations(
                    &mut program,
                    &tape,
                    Some(1_000_000),
                    checked,
                );
                let row = match attempt {
                    Ok(observed) => {
                        assert_eq!(observed.output, expected);
                        assert_eq!(observed.exit_code, Some(status));
                        assert!(
                            observed.failures.is_empty()
                                && observed.blocked.is_none()
                                && observed.signaled.is_none()
                        );
                        json!({"id":id,"engine":engine,"status":"PASS"})
                    }
                    Err(error) => {
                        let message = error.to_string();
                        incomplete.push(format!("{id}: {message}"));
                        json!({"id":id,"engine":engine,"status":"INCOMPLETE","error":message})
                    }
                };
                eprintln!("SHEET_PRODUCTION_REPLAY {row}");
                rows.push(row);
                assert_eq!(fs::read(&tape).unwrap(), bytes);
                assert!(!dir.join("work").exists());
            }
        }
        assert_eq!(
            rows.len(),
            if fresh { 12 } else { 18 },
            "nonzero complete selection"
        );
        fs::write(
            evidence.join(if fresh {
                "fresh-host-replays.json"
            } else {
                "production-checked-replay.json"
            }),
            serde_json::to_vec_pretty(&rows).unwrap(),
        )
        .unwrap();
        assert!(incomplete.is_empty(), "INCOMPLETE {incomplete:?}");
    });
}

#[test]
#[ignore = "requires SHEET_EVAL_31_PRODUCTION with immutable tapes and linked native executables"]
fn sheet_eval_31_production_refusal_controls() {
    hirsel_engine::with_compiler_stack(|| {
        let evidence = PathBuf::from(
            std::env::var_os("SHEET_EVAL_31_PRODUCTION").expect("production evidence"),
        );
        let scratch = scratch("sheet-refusals-");
        let path = package(scratch.path());
        for name in ["main", "formula", "cells", "graph", "evaluate", "store"] {
            fs::copy(
                evidence.join(format!("qualified-app/{name}.hir")),
                scratch.path().join(format!("{name}.hir")),
            )
            .unwrap();
        }

        let original = evidence.join("production/llvm-success-1/recording");
        let bytes = fs::read(&original).unwrap();
        let recorded = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
        let mut controls = vec![];
        let mut changed = recorded.clone();
        let request = changed
            .events
            .iter_mut()
            .find_map(|e| match &mut e.request {
                hirsel_rt::io::Request::ReadStdin { max, .. } => Some(max),
                _ => None,
            })
            .unwrap();
        *request += 1;
        controls.push(("request", changed));
        let mut trailing = recorded.clone();
        trailing.events.push(recorded.events[0].clone());
        controls.push(("trailing", trailing));
        let mut config = recorded.clone();
        config.arguments.push("changed".into());
        controls.push(("config", config));
        for (id, record) in controls {
            let tape = scratch.path().join(id);
            fs::write(&tape, hirsel_rt::launch::render_recording(&record).unwrap()).unwrap();
            for checked in [false, true] {
                let attempt = if checked {
                    hirsel_driver::replay_observations_checked(&path, &tape, Some(1_000_000))
                } else {
                    hirsel_driver::replay_observations(&path, &tape, Some(1_000_000))
                };
                let error = attempt.unwrap_err().to_string();
                assert!(
                    !error.contains("Steps"),
                    "unexpected tooling substitute {error}"
                );
                eprintln!("SHEET_REFUSAL_PASS {id} checked={checked}: {error}");
            }
            for backend in ["llvm", "cranelift"] {
                let out = std::process::Command::new(evidence.join(format!("app-{backend}")))
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .output()
                    .unwrap();
                assert_eq!(out.status.code(), Some(3), "{id}/{backend}: {out:?}");
                assert!(!out.stderr.is_empty());
                eprintln!("SHEET_REFUSAL_PASS {id} {backend}");
            }
        }
        // A changed source must refuse before replay, even when request prefixes match.
        let main = scratch.path().join("main.hir");
        let source = fs::read_to_string(&main).unwrap();
        fs::write(main, source.replace("512", "511")).unwrap();
        for checked in [false, true] {
            let attempt = if checked {
                hirsel_driver::replay_observations_checked(&path, &original, Some(1_000_000))
            } else {
                hirsel_driver::replay_observations(&path, &original, Some(1_000_000))
            };
            let error = attempt.unwrap_err().to_string();
            assert!(!error.contains("Steps"));
            eprintln!("SHEET_REFUSAL_PASS changed-source checked={checked}: {error}");
        }
        assert_eq!(fs::read(original).unwrap(), bytes);
    });
}

#[test]
fn sheet_eval_31_ast_preflight_reference_checked() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch("sheet-ast-");
        let path = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for case in extra_cases()
            .into_iter()
            .filter(|case| case["id"].as_str().unwrap().starts_with("ast-"))
        {
            let f = fixture(&case);
            for checked in [false, true] {
                let execution = if checked {
                    run_observed_checked_bounded(&mut loaded.engine, &run, (&f).into(), 1_000_000)
                } else {
                    run_observed_snapshot_bounded(&mut loaded.engine, &run, (&f).into(), 1_000_000)
                }
                .unwrap();
                let observed = execution.observation.expect("AST observation");
                assert_eq!(observed.output, case["stdout"].as_str().unwrap().as_bytes());
                assert_eq!(
                    observed.exit_code,
                    Some(case["exit"].as_i64().unwrap() as i32)
                );
                assert!(observed.failures.is_empty());
                eprintln!("SHEET_AST_PASS {} checked={checked}", case["id"]);
            }
        }
    });
}

#[test]
fn sheet_eval_31_scripted_ordered_widths() {
    native_cases(vec![
        json!({"id":"ordered-success","script":"set A1 = 2\nset B1 = A1+3\nshow\n","stdout":"OK\nOK\nA1 = 2\nB1 = 5\n","exit":0,"steps":1_000_000,"widths":true}),
        json!({"id":"ordered-domain","script":"set A1 = 1/0\nshow A1\n","stdout":"OK\nA1 = #DIV_ZERO\n","exit":1,"steps":1_000_000,"widths":true}),
        json!({"id":"ordered-cancel","script":"show\n","stdout":"error: canceled\n","exit":3,"steps":1_000_000,"widths":true,"cancel":true}),
    ]);
}

#[test]
fn sheet_eval_31_afterwrite_full_entry() {
    native_cases(vec![
        json!({"id":"afterwrite","script":"set A1 = 2+3*4\nshow A1\n","stdout":"OK\nA1 = 14\n","exit":0,"steps":1_000_000,"release":true}),
    ]);
}

#[test]
fn sheet_eval_31_reduced_dense_entry() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch("sheet-reduced-");
        let path = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let rows: Vec<_> = (1..=2)
            .flat_map(|r| {
                (b'A'..=b'P')
                    .map(move |c| json!({"name":format!("{}{r}",c as char),"formula":"SUM(A1:P1)"}))
            })
            .collect();
        let case = json!({"id":"reduced-512edges","script":"load big.json\nshow A1\nclear A1\nshow A1\n","files":{"big.json":json!({"version":1,"cells":rows}).to_string()},"stdout":"OK\nA1 = #CYCLE\nOK\nA1 = 0\n","exit":1,"steps":16_000_000});
        let f = fixture(&case);
        for checked in [false, true] {
            let execution = if checked {
                run_observed_checked_bounded(&mut loaded.engine, &run, (&f).into(), 16_000_000)
            } else {
                run_observed_snapshot_bounded(&mut loaded.engine, &run, (&f).into(), 16_000_000)
            }
            .unwrap();
            eprintln!(
                "SHEET_REDUCED_USAGE checked={checked} {:?}",
                execution.step_usage
            );
            let o = execution.observation.expect("reduced twin observation");
            assert_eq!(o.output, case["stdout"].as_str().unwrap().as_bytes());
            assert_eq!(o.exit_code, Some(1));
            assert!(o.failures.is_empty());
            eprintln!("SHEET_REDUCED_PASS checked={checked}; original4096/4097 remains INCOMPLETE");
        }
    });
}

#[test]
fn sheet_eval_31_unicode_formula_bounds() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch("sheet-unicode-");
        let path = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for case in extra_cases().into_iter().filter(|case| {
            case["id"].as_str().unwrap().starts_with("unicode-")
                || ["jobs-1", "jobs-2", "jobs-3"]
                    .iter()
                    .any(|id| case["id"] == *id)
        }) {
            let f = fixture(&case);
            for checked in [false, true] {
                let execution = if checked {
                    run_observed_checked_bounded(&mut loaded.engine, &run, (&f).into(), 1_000_000)
                } else {
                    run_observed_snapshot_bounded(&mut loaded.engine, &run, (&f).into(), 1_000_000)
                }
                .unwrap();
                let o = execution.observation.expect("Unicode observation");
                assert_eq!(o.output, case["stdout"].as_str().unwrap().as_bytes());
                assert_eq!(o.exit_code, Some(case["exit"].as_i64().unwrap() as i32));
                assert!(o.failures.is_empty());
                eprintln!("SHEET_UNICODE_PASS {} checked={checked}", case["id"]);
            }
        }
    });
}

#[test]
fn sheet_eval_31_canceled_recalculation() {
    native_cases(vec![
        json!({"id":"cancel-recalc","script":"","stdout":"canceled recalculation; cache preserved\n","exit":0,"steps":1_000_000,"recalc_cancel":true}),
    ]);
}

#[test]
#[ignore = "requires SHEET_EVAL_31_PRODUCTION with the retained original dense recordings"]
fn sheet_eval_31_remaining_dense_native_replay() {
    hirsel_engine::with_compiler_stack(|| {
        let evidence =
            PathBuf::from(std::env::var_os("SHEET_EVAL_31_PRODUCTION").expect("evidence"));
        let path = evidence.join("qualified-app/hirsel.json");
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let sites = Sites::new(validated.snapshot());
        let scratch = evidence.join("dense-native-replay");
        fs::create_dir_all(&scratch).unwrap();
        let mut tools = Toolchain::in_tree(&scratch.join("runtime"), &[&ready.target]).unwrap();
        tools.execution = Execution::Executable;
        fs::write(
            scratch.join("capped"),
            include_str!("../../../../tools/capped"),
        )
        .unwrap();
        fs::write(
            scratch.join("launch.py"),
            include_str!("../support/sheet_eval_31/launch.py"),
        )
        .unwrap();
        tools.wrapper = vec![
            "/usr/bin/python3".into(),
            scratch.join("launch.py").to_string_lossy().into_owned(),
        ];
        let root = PathBuf::from(
            std::env::var_os("SHEET_EVAL_31_DENSE_ROOT").expect("retained dense fixture root"),
        );
        let tape = root.join("edges-4096-Cranelift/recording");
        let bytes = fs::read(&tape).unwrap();
        let case = extra_cases()
            .into_iter()
            .find(|case| case["id"] == "edges-4096")
            .unwrap();
        let mut f = fixture(&case);
        f.files.clear();
        f.file_reads.clear();
        for backend in ["Llvm", "Cranelift"] {
            let dir = root.join(format!("edges-4096-{backend}"));
            let runnable = Runnable::Executable(dir.join("program"));
            let launched = launch_fixture(
                &runnable,
                &ready.target,
                &tools,
                &dir,
                true,
                Tape::Replay(&tape),
                &f,
            )
            .unwrap();
            let observed = observe(&launched, &ready, &sites).unwrap();
            assert_eq!(observed.output, case["stdout"].as_str().unwrap().as_bytes());
            assert_eq!(observed.exit_code, Some(1));
            assert!(observed.failures.is_empty());
            eprintln!("SHEET_REMAINING_DENSE_REPLAY_PASS {backend}");
        }
        assert_eq!(fs::read(tape).unwrap(), bytes);
    });
}

#[test]
#[ignore = "requires SHEET_EVAL_31_PRODUCTION with the immutable real ENOSPC stdout recordings"]
fn sheet_eval_31_real_output_error_replay() {
    hirsel_engine::with_compiler_stack(|| {
        let root = PathBuf::from(std::env::var_os("SHEET_EVAL_31_PRODUCTION").expect("evidence"));
        let path = root.join("qualified-app/hirsel.json");
        let mut program = hirsel_driver::load_program(&path).unwrap();
        for recorder in ["llvm", "cranelift"] {
            let tape = root.join(format!("output-fault-{recorder}/recording"));
            let bytes = fs::read(&tape).unwrap();
            for checked in [false, true] {
                let observed = hirsel_driver::replay_loaded_observations(
                    &mut program,
                    &tape,
                    Some(1_000_000),
                    checked,
                )
                .unwrap();
                assert!(observed.output.is_empty() && observed.failures.is_empty());
                assert_eq!(observed.exit_code, Some(3));
                eprintln!("SHEET_REAL_OUTPUT_REPLAY_PASS {recorder} checked={checked}");
            }
            for backend in ["llvm", "cranelift"] {
                let out = std::process::Command::new(root.join(format!("app-{backend}")))
                    .arg("--hirsel-replay")
                    .arg(&tape)
                    .output()
                    .unwrap();
                assert_eq!(out.status.code(), Some(3));
                assert!(out.stdout.is_empty() && out.stderr.is_empty());
                eprintln!("SHEET_REAL_OUTPUT_REPLAY_PASS {recorder} {backend}");
            }
            assert_eq!(fs::read(tape).unwrap(), bytes);
        }
    });
}
