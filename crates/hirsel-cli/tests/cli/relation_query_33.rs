//! Full Io+Args entry, independent frozen SQLite/model vectors, immutable tapes.
use hirsel_driver::native::*;
use hirsel_engine::{RunConfiguration, SimulationConfig};
use hirsel_run_schema::{Access, FileReadScript};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

fn root(label: &str) -> PathBuf {
    let base = std::env::var_os("RELATION_QUERY_33_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix(&format!("relation-query-33-{label}-"))
        .tempdir_in(base)
        .unwrap()
        .keep()
}
fn package(root: &Path) -> PathBuf {
    for (name, data) in [
        (
            "main.hir",
            include_str!("../../../../examples/relquery/main.hir"),
        ),
        (
            "schema.hir",
            include_str!("../../../../examples/relquery/schema.hir"),
        ),
        (
            "plan.hir",
            include_str!("../../../../examples/relquery/plan.hir"),
        ),
        (
            "execute.hir",
            include_str!("../../../../examples/relquery/execute.hir"),
        ),
        (
            "aggregate.hir",
            include_str!("../../../../examples/relquery/aggregate.hir"),
        ),
        (
            "output.hir",
            include_str!("../../../../examples/relquery/output.hir"),
        ),
        (
            "cases.json",
            include_str!("../support/relation_query_33/cases.json"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
        (
            "limits.json",
            include_str!("../support/relation_query_33/limits.json"),
        ),
        (
            "supplemental.json",
            include_str!("../support/relation_query_33/supplemental.json"),
        ),
    ] {
        fs::write(root.join(name), data).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/relquery/hirsel.json")).unwrap();
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
fn cases() -> Vec<Value> {
    let mut rows =
        serde_json::from_str::<Value>(include_str!("../support/relation_query_33/cases.json"))
            .unwrap()["cases"]
            .as_array()
            .unwrap()
            .clone();
    rows.extend(
        serde_json::from_str::<Value>(include_str!(
            "../support/relation_query_33/supplemental.json"
        ))
        .unwrap()["cases"]
            .as_array()
            .unwrap()
            .clone(),
    );
    rows.extend(
        serde_json::from_str::<Value>(include_str!("../support/relation_query_33/limits.json"))
            .unwrap()["cases"]
            .as_array()
            .unwrap()
            .clone(),
    );
    rows
}
fn selected(row: &Value) -> bool {
    std::env::var("RELATION_QUERY_33_CASE").map_or(true, |s| s.split(',').any(|id| row["id"] == id))
}
fn fixture(row: &Value, jobs: u64) -> Fixture {
    let mut files: BTreeMap<String, Vec<u8>> = row["files"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (format!("data/{k}"), v.as_str().unwrap().as_bytes().to_vec()))
        .collect();
    files.insert(
        "data/plan.json".into(),
        serde_json::to_vec(&row["plan"]).unwrap(),
    );
    for q in row["plan"]["queries"].as_array().unwrap() {
        files.insert(
            format!("data/{}.csv", q["name"].as_str().unwrap()),
            b"preserve\n".to_vec(),
        );
    }
    let mut f = Fixture {
        args: vec![
            row["command"].as_str().unwrap_or("run").into(),
            "data".into(),
            "plan.json".into(),
            row["jobs"].as_u64().unwrap_or(jobs).to_string(),
        ],
        files,
        mounts: BTreeMap::from([(
            "data".into(),
            if row["access"] == "read" {
                Access::Read
            } else {
                Access::ReadWrite
            },
        )]),
        steps: Some(1_000_000),
        ..Default::default()
    };
    if row["pending"] == true {
        f.args.push("--interruptible".into());
        f.file_reads
            .insert("data/plan.json".into(), FileReadScript::Pending);
        f.signals = Some(
            serde_json::from_value(
                json!({"provider":"signals.script@1","kinds":["interrupt"],"script":["interrupt"]}),
            )
            .unwrap(),
        );
    }
    f
}
fn verify(row: &Value, observed: &hirsel_interp::observation::Observable, fixture: &Fixture) {
    assert_eq!(
        observed.exit_code,
        row["exit"].as_i64().map(|n| n as i32),
        "{}: {observed:?}",
        row["id"]
    );
    assert_eq!(
        observed.output,
        row["stdout"].as_str().unwrap().as_bytes(),
        "{}",
        row["id"]
    );
    assert!(observed.failures.is_empty(), "{observed:?}");
    assert!(observed.blocked.is_none());
    let state = observed.final_state.as_ref().unwrap();
    assert_eq!(
        state.stderr,
        row["stderr"].as_str().unwrap().as_bytes(),
        "{}",
        row["id"]
    );
    let mut expected = fixture.files.clone();
    for (k, v) in row["expected"].as_object().unwrap() {
        expected.insert(format!("data/{k}"), v.as_str().unwrap().as_bytes().to_vec());
    }
    assert_eq!(
        state.trees[&0].1, expected,
        "{} files/atomic cleanup",
        row["id"]
    );
    if row["pending"] == true {
        assert!(observed.effects.iter().any(|e|matches!(&e.effect,hirsel_interp::observation::Effect::CapabilityConsumed(e) if e.operation=="file.read.pending")),"never pended");
    }
}
#[test]
fn relation_query_33_reference_checked_vectors() {
    hirsel_engine::with_compiler_stack(|| {
        let root = root("simulation");
        let manifest = package(&root);
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap_or_else(|e| panic!("{e}"));
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
        let mut count = 0;
        for row in cases().iter().filter(|r| selected(r)) {
            for jobs in [1, 2, 4] {
                // Width semantics are independent from worker capacity; all original
                // caps run once, ordinary independent workflows cover every jobs value.
                if row["id"].as_str().unwrap().starts_with("cap-") && jobs != 1 {
                    continue;
                }
                let fixture = fixture(row, jobs);
                let config: SimulationConfig = (&fixture).into();
                let mut previous = None;
                for checked in [false, true] {
                    count += 1;
                    let id = format!(
                        "{}-j{jobs}-{}",
                        row["id"],
                        if checked { "checked" } else { "reference" }
                    );
                    eprintln!("RELQUERY_STARTED {id}");
                    let result = if checked {
                        hirsel_engine::run_observed_checked_bounded(
                            &mut loaded.engine,
                            &run,
                            config.clone(),
                            1_000_000,
                        )
                    } else {
                        hirsel_engine::run_observed_snapshot_bounded(
                            &mut loaded.engine,
                            &run,
                            config.clone(),
                            1_000_000,
                        )
                    };
                    let mut completed = false;
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let result = result.unwrap();
                        if steps_case(row) {
                            assert!(result.observation.is_none());
                            assert!(
                                matches!(&result.outcome, Ok(hirsel_engine::RunOutcome::ToolingLimit(limit)) if limit.kind == "Steps" && limit.limit == 1_000_000),
                                "{:?}",
                                result.outcome
                            );
                            assert_eq!(result.step_usage.unwrap().consumed, 1_000_000);
                            return;
                        }
                        let Some(observed) = result.observation else {
                            panic!("INCOMPLETE: {:?}", result.outcome)
                        };
                        verify(row, &observed, &fixture);
                        completed = true;
                        if let Some(previous) = &previous {
                            assert_eq!(&observed, previous, "full observation");
                        }
                        previous = Some(observed);
                    }));
                    let passed = result.is_ok();
                    receipts
                        .push(json!({"cell":id,"selected":true,"started":true,"completed":completed,"passed":passed && completed,"disposition":if completed {"completed"} else {"INCOMPLETE Steps 1000000"}}));
                    fs::write(
                        root.join("receipts.json"),
                        serde_json::to_vec_pretty(&receipts).unwrap(),
                    )
                    .unwrap();
                    if !passed {
                        failures.push(id);
                    } else {
                        eprintln!(
                            "RELQUERY_{} {id}",
                            if completed {
                                "PASS"
                            } else {
                                "INCOMPLETE Steps 1000000"
                            }
                        );
                    }
                }
            }
        }
        assert!(count > 0);
        assert!(failures.is_empty(), "{}: {failures:?}", root.display());
    });
}
#[test]
fn relation_query_33_full_entry_native_replay() {
    hirsel_engine::with_compiler_stack(|| {
        let resume = std::env::var_os("RELATION_QUERY_33_RESUME_ROOT");
        let root = resume
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| root("native"));
        let manifest = if resume.is_some() {
            root.join("hirsel.json")
        } else {
            package(&root)
        };
        let mut progress: BTreeSet<String> = fs::read(root.join("progress.json"))
            .ok()
            .map(|v| serde_json::from_slice(&v).unwrap())
            .unwrap_or_default();
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap_or_else(|e| panic!("{e}"));
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
        // Embed the reviewed helper so remote and host executions use identical bytes.
        tools.wrapper = vec![
            "env".into(),
            "HIRSEL_MEMORY_LIMIT=2G".into(),
            "HIRSEL_JOBS=1".into(),
            "PATH=/usr/bin:/bin".into(),
            "/usr/bin/python3".into(),
            root.join("capped").to_str().unwrap().into(),
            "--timeout".into(),
            "60".into(),
            "prlimit".into(),
            "--cpu=60:61".into(),
        ];
        tools.launch_timeout = std::time::Duration::from_secs(75);
        fs::write(root.join("toolchain.txt"), format!("{tools:#?}")).unwrap();
        for (key, path) in &tools.runtime {
            let destination = root.join(format!("runtime-{}-{:?}.a", key.0, key.1));
            fs::copy(path, &destination).unwrap();
        }
        if let Some(image) = std::env::var_os("HIRSEL_IMAGE") {
            fs::copy(image, root.join("std-image")).unwrap();
        }

        let ready = build(&input, &options).unwrap();
        let sites = Sites::new(input.snapshot());
        let backend_objects = [Backend::Llvm, Backend::Cranelift]
            .into_iter()
            .map(|backend| {
                let objects = emit_backend_objects(
                    &ready,
                    backend,
                    OptLevel::None,
                    Partition::Whole,
                    false,
                    None,
                )
                .unwrap();
                (backend, objects)
            })
            .collect::<Vec<_>>();
        let mut receipts = Vec::new();
        let native_jobs =
            std::env::var("RELATION_QUERY_33_JOBS").map_or(2, |v| v.parse::<u64>().unwrap());
        assert!((1..=4).contains(&native_jobs));
        let mut count = 0;
        for row in cases().iter().filter(|r| selected(r)) {
            count += 1;
            let id = row["id"].as_str().unwrap();
            let fixture = fixture(row, native_jobs);
            let observation = Observation {
                exit_code: row["exit"].as_i64().map(|n| n as i32),
                blocked: false,
                budget: None,
                signaled: None,
                output: row["stdout"].as_str().unwrap().as_bytes().to_vec(),
                failures: vec![],
                roots: vec![],
            };
            let dir = root.join(id);
            fs::create_dir_all(&dir).unwrap();
            // Original interpreter cells are retained in the vector test; use the
            // independently frozen expected status/stdout even for Steps cells.
            if ["many-to-many", "overflow", "denial", "cancel"].contains(&id)
                && !progress.contains(&format!("{id}/differential"))
            {
                for (name, outcome) in differential_in(
                    &input,
                    &loaded.entry,
                    &observation,
                    &matrix,
                    &tools,
                    &dir,
                    &fixture,
                ) {
                    assert_eq!(outcome, Outcome::Agree, "{id} {name}");
                }
                accept(&root, &mut progress, format!("{id}/differential"));
            }
            let mut runnables = Vec::new();
            for (backend, objects) in &backend_objects {
                let dir = dir.join(format!("{backend:?}"));
                fs::create_dir_all(&dir).unwrap();
                let runnable =
                    link_fixture_runnable(&ready, &loaded.entry, objects, &tools, &dir, &fixture)
                        .unwrap();
                runnables.push((runnable, dir));
            }
            for (recorder, dir) in &runnables {
                let tape = dir.join("recording.tape");
                let cell = format!("{id}/{}", dir.file_name().unwrap().to_str().unwrap());
                if !progress.contains(&format!("{cell}/record")) {
                    eprintln!("RELQUERY_STARTED {cell}/record");
                    let recorded = launch_fixture(
                        recorder,
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
                    assert_eq!(
                        observe(&recorded, &ready, &sites).unwrap(),
                        observation,
                        "{id}"
                    );
                    accept(&root, &mut progress, format!("{cell}/record"));
                }
                let frozen = fs::read(&tape).unwrap();
                let record = hirsel_rt::launch::parse_recording_bytes(&frozen).unwrap();
                assert_eq!(record.arguments, fixture.args);
                assert_eq!(record.tree.files, fixture.files);
                record.bound_run().unwrap();
                verify_tape(row, &record);
                if row["pending"] == true {
                    assert!(record.events.iter().any(|e| matches!(
                        (&e.request, &e.completion),
                        (
                            hirsel_rt::io::Request::FileRead { .. },
                            hirsel_rt::io::Completion::Blocked { committed: false }
                        )
                    )));
                }
                let replay_fixture = Fixture {
                    files: BTreeMap::new(),
                    file_reads: BTreeMap::new(),
                    ..fixture.clone()
                };
                for (runnable, replay_dir) in &runnables {
                    let key = format!(
                        "{cell}/native-replay-{}",
                        replay_dir.file_name().unwrap().to_str().unwrap()
                    );
                    if progress.contains(&key) {
                        continue;
                    }
                    eprintln!("RELQUERY_STARTED {key}");
                    let replayed = launch_fixture(
                        runnable,
                        &ready.target,
                        &tools,
                        replay_dir,
                        true,
                        Tape::Replay(&tape),
                        &replay_fixture,
                    )
                    .unwrap();
                    assert_eq!(
                        observe(&replayed, &ready, &sites).unwrap(),
                        observation,
                        "{id} native replay"
                    );
                    accept(&root, &mut progress, key);
                }
                for (index, replay) in [
                    hirsel_driver::replay_observations,
                    hirsel_driver::replay_observations_checked,
                ]
                .into_iter()
                .enumerate()
                {
                    let engine = if index == 0 { "reference" } else { "checked" };
                    let key = format!("{cell}/{engine}");
                    if progress.contains(&key) {
                        continue;
                    }
                    eprintln!("RELQUERY_STARTED {key}");
                    let replayed = replay(&manifest, &tape, Some(1_000_000));
                    if steps_case(row) {
                        let error = replayed.unwrap_err().to_string();
                        assert!(
                            error.contains("ToolingLimit")
                                && error.contains("Steps")
                                && error.contains("limit: 1000000"),
                            "{error}"
                        );
                        eprintln!("RELQUERY_INCOMPLETE {id} host replay: Steps 1000000");
                    } else {
                        let replayed = replayed.unwrap();
                        assert_eq!(Observation::of_reference(&replayed), observation);
                        // Host replay intentionally exposes no simulated tree.
                        // Exact stderr/files were checked from this consumed tape.
                        assert!(replayed.failures.is_empty());
                        assert!(replayed.blocked.is_none());
                        assert!(replayed.final_state.is_none());
                        accept(&root, &mut progress, key);
                    }
                }
                assert_eq!(fs::read(&tape).unwrap(), frozen);
                if id == "many-to-many"
                    && native_jobs == 2
                    && !progress.contains(&format!("{cell}/refusals"))
                {
                    refusals(
                        &manifest,
                        &tape,
                        &record,
                        &runnables,
                        &ready,
                        &tools,
                        &fixture,
                        &observation,
                        &root,
                        &mut progress,
                    );
                    accept(&root, &mut progress, format!("{cell}/refusals"));
                }
                eprintln!("RELQUERY_NATIVE_PASS {id} {}", dir.display());
                receipts.push(json!({"id":id,"tape":tape,"native_passed":true,"interpreter_replay_completed":!steps_case(row)}));
                fs::write(
                    root.join("receipts.json"),
                    serde_json::to_vec_pretty(&receipts).unwrap(),
                )
                .unwrap();
            }
        }
        assert!(count > 0);
    });
}

fn verify_tape(row: &Value, tape: &hirsel_rt::launch::Recording) {
    use hirsel_rt::io::{Completion as C, Request as R};
    let stderr: Vec<u8> = tape
        .events
        .iter()
        .filter_map(|e| match &e.request {
            R::PrintStderr { bytes, .. } => Some(bytes.as_slice()),
            _ => None,
        })
        .flatten()
        .copied()
        .collect();
    assert_eq!(stderr, row["stderr"].as_str().unwrap().as_bytes());
    let mut open: BTreeMap<u64, (String, Vec<u8>)> = BTreeMap::new();
    let mut published = BTreeMap::new();
    for e in &tape.events {
        match (&e.request, &e.completion) {
            (R::CreateAtomic { path, .. }, C::Handle(Ok(id))) => {
                open.insert(*id, (path.clone(), vec![]));
            }
            (R::WriteAll { file, bytes, .. }, C::Unit(Ok(()))) => {
                open.get_mut(file).unwrap().1.extend(bytes)
            }
            (R::Commit { file, .. }, C::Unit(Ok(()))) => {
                let (path, bytes) = open.remove(file).unwrap();
                published.insert(path, bytes);
            }
            _ => {}
        }
    }
    let expected: BTreeMap<String, Vec<u8>> = row["expected"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().as_bytes().to_vec()))
        .collect();
    assert_eq!(published, expected, "native atomic publication requests");
    assert!(open.is_empty(), "unfinished atomic files");
}

fn steps_case(row: &Value) -> bool {
    matches!(
        row["id"].as_str().unwrap(),
        "cap-pairs-4096" | "cap-output-65536" | "cap-output-65537" | "cap-input-65536"
    )
}

#[allow(clippy::too_many_arguments)]
fn refusals(
    manifest: &Path,
    original: &Path,
    record: &hirsel_rt::launch::Recording,
    runnables: &[(Runnable, PathBuf)],
    ready: &ReadyProgram,
    tools: &Toolchain,
    fixture: &Fixture,
    observation: &Observation,
    root: &Path,
    progress: &mut BTreeSet<String>,
) {
    use hirsel_rt::io::Request;
    let mut binding = record.clone();
    binding.arguments.push("changed".into());
    let mut request = record.clone();
    let directory = request
        .events
        .iter_mut()
        .find_map(|e| match &mut e.request {
            Request::Directory { path, .. } => Some(path),
            _ => None,
        })
        .unwrap();
    directory.push_str("_changed");
    let mut missing = record.clone();
    missing.events.pop().unwrap();
    let mut extra = record.clone();
    extra.events.push(record.events[0].clone());
    for (kind, tape) in [
        ("header disagrees", binding),
        ("ReplayMismatch", request),
        ("MissingInput", missing),
        ("UnconsumedInput", extra),
    ] {
        let path = original.with_file_name(format!("refusal-{kind}.tape"));
        let rendered = hirsel_rt::launch::render_recording(&tape).unwrap();
        if path.exists() {
            assert_eq!(fs::read(&path).unwrap(), rendered.as_bytes());
        } else {
            fs::write(&path, rendered).unwrap();
        }
        let base = format!(
            "many-to-many/{}/refusal/{kind}",
            original
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
        );
        for (index, replay) in [
            hirsel_driver::replay_observations,
            hirsel_driver::replay_observations_checked,
        ]
        .into_iter()
        .enumerate()
        {
            let key = format!("{base}/host-{index}");
            if progress.contains(&key) {
                continue;
            }
            let error = replay(manifest, &path, Some(1_000_000))
                .unwrap_err()
                .to_string();
            assert!(error.contains(kind), "{kind}: {error}");
            accept(root, progress, key);
        }
        for (runnable, dir) in runnables {
            let key = format!("{base}/{}", dir.file_name().unwrap().to_str().unwrap());
            if progress.contains(&key) {
                continue;
            }
            let refused = launch_fixture(
                runnable,
                &ready.target,
                tools,
                dir,
                true,
                Tape::Replay(&path),
                fixture,
            );
            match refused {
                Err(error) => assert!(error.to_string().contains(kind), "{kind}: {error}"),
                Ok(output) => {
                    assert_eq!(output.exit, Some(3), "{kind}: {output:?}");
                    assert!(
                        String::from_utf8_lossy(&output.stderr).contains(kind)
                            || output.record.contains(kind)
                            || fs::read_to_string(dir.join("refusal"))
                                .unwrap_or_default()
                                .contains(kind),
                        "{kind}: {output:?}"
                    );
                    let expected = if kind == "UnconsumedInput" {
                        observation.output.as_slice()
                    } else {
                        b""
                    };
                    assert_eq!(
                        output.stdout, expected,
                        "only the exact valid recorded prefix is allowed"
                    );
                }
            }
            accept(root, progress, key);
        }
    }
}

fn accept(root: &Path, progress: &mut BTreeSet<String>, key: String) {
    eprintln!("RELQUERY_PASS {key}");
    progress.insert(key);
    fs::write(
        root.join("progress.json"),
        serde_json::to_vec_pretty(progress).unwrap(),
    )
    .unwrap();
}
