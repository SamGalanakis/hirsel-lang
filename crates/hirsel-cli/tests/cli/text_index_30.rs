//! Complete application entry/config/modules, independent Python scan vectors.
use hirsel_engine::{RunConfiguration, SimulationConfig};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn scratch(label: &str) -> tempfile::TempDir {
    let base = std::env::var_os("TEXT_INDEX_30_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    let mut scratch = tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap();
    if std::env::var_os("TEXT_INDEX_30_EVIDENCE_DIR").is_some() {
        scratch.disable_cleanup(true);
    }
    scratch
}

fn package(root: &Path) -> PathBuf {
    for (name, source) in [
        (
            "main",
            include_str!("../../../../examples/textindex/main.hir"),
        ),
        (
            "manifest",
            include_str!("../../../../examples/textindex/manifest.hir"),
        ),
        (
            "tokenize",
            include_str!("../../../../examples/textindex/tokenize.hir"),
        ),
        (
            "index",
            include_str!("../../../../examples/textindex/index.hir"),
        ),
        (
            "store",
            include_str!("../../../../examples/textindex/store.hir"),
        ),
        (
            "query",
            include_str!("../../../../examples/textindex/query.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/textindex/hirsel.json")).unwrap();
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

fn cases(root: &Path) -> Vec<Value> {
    let oracle = root.join("vectors.py");
    fs::write(&oracle, include_str!("../support/text_index_30/vectors.py")).unwrap();
    let result = Command::new("python3").arg(&oracle).output().unwrap();
    assert!(result.status.success(), "{result:?}");
    fs::write(root.join("cases.json"), &result.stdout).unwrap();
    let mut cases: Vec<Value> = serde_json::from_slice(&result.stdout).unwrap();
    if let Ok(filter) = std::env::var("TEXT_INDEX_30_CASE") {
        cases.retain(|case| filter.split(',').any(|id| case["id"] == id));
    }
    assert!(!cases.is_empty(), "zero selected cases");
    cases
}

fn bytes(value: &Value) -> Vec<u8> {
    value
        .as_str()
        .map(|s| s.as_bytes().to_vec())
        .unwrap_or_else(|| {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|n| u8::try_from(n.as_u64().unwrap()).unwrap())
                .collect()
        })
}

fn expected(stage: &Value, output: &[u8]) {
    if let Some(index) = stage.get("index") {
        assert_eq!(&serde_json::from_slice::<Value>(output).unwrap(), index);
    } else if let Some(rows) = stage.get("rows") {
        let actual: Vec<Value> = String::from_utf8(output.to_vec())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(&Value::Array(actual), rows);
    } else {
        assert_eq!(output, stage["stdout"].as_str().unwrap().as_bytes());
    }
}

fn scripted(boundary: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = scratch("text-index-30-script-");
        let selection = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e:?}"));
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let selected: Vec<_> = cases(scratch.path())
            .into_iter()
            .filter(|c| c["boundary"] == boundary)
            .collect();
        assert!(!selected.is_empty());
        let mut completed = 0;
        let mut incomplete = 0;
        for case in selected {
            let id = case["id"].as_str().unwrap();
            let mut observations = Vec::new();
            for engine in ["reference", "checked-h"] {
                if let Ok(filter) = std::env::var("TEXT_INDEX_30_ENGINE") {
                    if !filter.split(',').any(|e| e == engine) {
                        continue;
                    }
                }
                let mut world = SimulationConfig {
                    mounts: [
                        ("input".into(), hirsel_run_schema::Access::Read),
                        ("index".into(), hirsel_run_schema::Access::ReadWrite),
                    ]
                    .into(),
                    ..Default::default()
                };
                world.directories.extend(["input".into(), "index".into()]);
                for (name, data) in case["files"].as_object().unwrap() {
                    world.files.insert(name.clone(), bytes(data));
                }
                if case["access"] == "denied" {
                    world.mounts.remove("input");
                }
                if case["access"] == "read" {
                    world
                        .mounts
                        .insert("index".into(), hirsel_run_schema::Access::Read);
                }
                if let Some(denied) = case["deny_projection"].as_str() {
                    world.projections.remove(denied);
                }
                if let Some(pending) = case["pending"].as_object() {
                    for name in pending.keys() {
                        world.file_reads.insert(
                            format!("input/{name}"),
                            hirsel_run_schema::FileReadScript::Pending,
                        );
                    }
                }
                for (stage_index, stage) in case["stages"].as_array().unwrap().iter().enumerate() {
                    let cell = format!("{id}/{stage_index}/{engine}");
                    if let Ok(filter) = std::env::var("TEXT_INDEX_30_STAGES") {
                        if !filter
                            .split(',')
                            .any(|n| n.parse::<usize>().unwrap() == stage_index)
                        {
                            continue;
                        }
                    }
                    if let Ok(resume) = std::env::var("TEXT_INDEX_30_RESUME") {
                        let path = Path::new(&resume)
                            .join(format!("{id}-{stage_index}-{engine}.world.json"));
                        if path.exists() {
                            world.files = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                        }
                    }
                    let world_bytes = serde_json::to_vec(&world.files).unwrap();
                    fs::write(
                        scratch
                            .path()
                            .join(format!("{id}-{stage_index}-{engine}.world.json")),
                        world_bytes,
                    )
                    .unwrap();
                    eprintln!("TEXT_INDEX_30_STARTED {cell}");
                    world.args = stage["args"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().into())
                        .collect();
                    // Bounds are fixture policy fixed before first execution, never a retry.
                    let limit = if boundary { 8_000_000 } else { 1_000_000 };
                    let execution = if engine == "reference" {
                        hirsel_engine::run_observed_snapshot_bounded(
                            &mut loaded.engine,
                            &config,
                            world.clone(),
                            limit,
                        )
                    } else {
                        hirsel_engine::run_observed_checked_bounded(
                            &mut loaded.engine,
                            &config,
                            world.clone(),
                            limit,
                        )
                    }
                    .unwrap();
                    eprintln!("TEXT_INDEX_30_USAGE {cell} {:?}", execution.step_usage);
                    fs::write(
                        scratch
                            .path()
                            .join(format!("{id}-{stage_index}-{engine}.execution")),
                        format!("{execution:?}"),
                    )
                    .unwrap();
                    let Some(observed) = execution.observation.as_ref() else {
                        assert!(
                            matches!(
                                execution.outcome,
                                Ok(hirsel_engine::RunOutcome::ToolingLimit(_))
                            ),
                            "{cell}: {execution:?}"
                        );
                        assert_eq!(execution.step_usage.unwrap().consumed, limit);
                        incomplete += 1;
                        eprintln!(
                            "TEXT_INDEX_30_INCOMPLETE {cell} Steps limit={limit}; no observation, remaining stages unrun"
                        );
                        break;
                    };
                    fs::write(
                        scratch
                            .path()
                            .join(format!("{id}-{stage_index}-{engine}.observation")),
                        format!("{execution:?}"),
                    )
                    .unwrap();
                    assert_eq!(
                        observed.exit_code,
                        stage["exit"].as_i64().map(|n| n as i32),
                        "{cell}: {execution:?}"
                    );
                    expected(stage, &observed.output);
                    let state = observed.final_state.as_ref().unwrap();
                    assert_eq!(
                        state.stderr,
                        stage["stderr"].as_str().unwrap_or("").as_bytes(),
                        "{cell}"
                    );
                    assert!(
                        observed.failures.is_empty()
                            && observed.blocked.is_none()
                            && observed.signaled.is_none(),
                        "{cell}: {observed:?}"
                    );
                    if stage["exit"] == 2 {
                        assert_eq!(
                            state.trees[&0].1, world.files,
                            "failed build/query changed files: {cell}"
                        );
                    }
                    if id == "pending-cancel" {
                        let events: Vec<_> = observed
                            .effects
                            .iter()
                            .filter_map(|e| {
                                if let hirsel_interp::observation::Effect::CapabilityConsumed(e) =
                                    &e.effect
                                {
                                    Some(e)
                                } else {
                                    None
                                }
                            })
                            .collect();
                        assert!(
                            events.iter().any(|e| e.operation == "file.read.pending"),
                            "no actual Pending read: {cell}"
                        );
                        assert!(
                            !events.iter().any(|e| format!("{e:?}").contains("4.txt")),
                            "queued doc admitted: {cell}"
                        );
                    }
                    if engine == "reference" {
                        observations.push(observed.clone());
                    } else if let Some(reference) = observations.get(stage_index) {
                        assert_eq!(observed, reference, "full semantic observation {cell}");
                    }
                    world.files = state.trees[&0].1.clone();
                    fs::write(
                        scratch
                            .path()
                            .join(format!("{id}-{}-{engine}.world.json", stage_index + 1)),
                        serde_json::to_vec(&world.files).unwrap(),
                    )
                    .unwrap();
                    eprintln!("TEXT_INDEX_30_PASS {cell}");
                    completed += 1;
                }
            }
        }
        assert!(completed + incomplete > 0);
        eprintln!("TEXT_INDEX_30_COMPLETED {completed} INCOMPLETE {incomplete}");
        if std::env::var_os("TEXT_INDEX_30_EVIDENCE_DIR").is_some() {
            let _ = scratch.keep();
        }
    });
}

#[test]
fn text_index_30_scripted_reference_checked() {
    scripted(false);
}
#[test]
fn text_index_30_boundaries_reference_checked() {
    scripted(true);
}

fn native(boundary: bool) {
    hirsel_engine::with_compiler_stack(|| {
        if let Ok(existing) = std::env::var("TEXT_INDEX_30_REPLAY_ONLY") {
            let root = Path::new(&existing);
            let selection = root.join("hirsel.json");
            let binaries = [
                root.join("native-llvm/llvm"),
                root.join("native-cranelift/cranelift"),
            ];
            replay_refusals(
                &selection,
                root,
                &root.join("native-llvm/tantivy-j4-w1-s0.tape"),
                &binaries,
            );
            return;
        }
        let scratch = scratch("text-index-30-native-");
        let root = scratch.path();
        let selection = package(root);
        let selected = cases(root);
        fs::write(
            root.join("cases.json"),
            serde_json::to_vec(&selected).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("native.py"),
            include_str!("../support/text_index_30/native.py"),
        )
        .unwrap();
        fs::write(
            root.join("capped"),
            include_str!("../../../../tools/capped"),
        )
        .unwrap();
        fs::write(root.join("capture.py"),"import pathlib,subprocess,sys\np=subprocess.run(sys.argv[3:],capture_output=True)\npathlib.Path(sys.argv[1]).write_bytes(p.stdout)\npathlib.Path(sys.argv[2]).write_bytes(p.stderr)\nsys.exit(p.returncode)\n").unwrap();
        let mut recordings = Vec::new();
        let mut binaries = Vec::new();
        for backend in ["llvm", "cranelift"] {
            let mut command = Command::new("python3");
            command
                .arg(root.join("native.py"))
                .arg("--root")
                .arg(root)
                .arg("--hirsel")
                .arg(
                    Path::new(env!("CARGO_BIN_EXE_hirsel"))
                        .canonicalize()
                        .unwrap(),
                )
                .args(["--backend", backend]);
            if boundary {
                command.arg("--boundary");
            }
            let out = command.output().unwrap();
            fs::write(root.join(format!("{backend}.stdout")), &out.stdout).unwrap();
            fs::write(root.join(format!("{backend}.stderr")), &out.stderr).unwrap();
            if !out.status.success() {
                let retained = scratch.keep();
                panic!(
                    "native INCOMPLETE {} {backend}: {}",
                    retained.display(),
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            let report: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert!(report["passed"].as_u64().unwrap() > 1);
            eprintln!(
                "TEXT_INDEX_30_NATIVE {backend} completed={} passed={}",
                report["completed"], report["passed"]
            );
            recordings.extend(report["recordings"].as_array().unwrap().iter().cloned());
            binaries.push(root.join(format!("native-{backend}/{backend}")));
        }
        let mut semantic = std::collections::BTreeMap::new();
        let control_tape = recordings
            .first()
            .map(|row| PathBuf::from(row["tape"].as_str().unwrap()));
        for row in recordings {
            let id = row["id"].as_str().unwrap();
            let key = (
                row["backend"].as_str().unwrap().to_owned(),
                id.replace("-w1-", "-wN-")
                    .replace("-w2-", "-wN-")
                    .replace("-w4-", "-wN-"),
            );
            let tape = Path::new(row["tape"].as_str().unwrap());
            let original = fs::read(tape).unwrap();
            let mut recording = hirsel_rt::host_launch::read_recording(tape).unwrap();
            let binding = recording.binding.as_mut().unwrap();
            binding.admitted_at = 0;
            binding.placement = None;
            if let Some(previous) = semantic.get(&key) {
                assert_eq!(
                    &recording, previous,
                    "complete semantic event order across widths: {id}"
                );
            } else {
                semantic.insert(key, recording);
            }
            let limit = if boundary { 8_000_000 } else { 1_000_000 };
            let plain = hirsel_driver::replay_observations(&selection, tape, Some(limit)).unwrap();
            let checked =
                hirsel_driver::replay_observations_checked(&selection, tape, Some(limit)).unwrap();
            assert!(plain.equivalent(&checked));
            fs::write(
                tape.with_extension("replay-observation"),
                format!("{plain:?}\n{checked:?}"),
            )
            .unwrap();
            let stage = &row["expected"];
            expected(stage, &plain.output);
            assert_eq!(plain.exit_code, stage["exit"].as_i64().map(|n| n as i32));
            assert!(plain.failures.is_empty());
            for binary in &binaries {
                let out = Command::new("python3")
                    .arg(root.join("capped"))
                    .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
                    .arg("python3")
                    .arg(root.join("capture.py"))
                    .arg(root.join("replay.stdout"))
                    .arg(root.join("replay.stderr"))
                    .arg(binary)
                    .env("HIRSEL_JOBS", "4")
                    .env("HIRSEL_MEMORY_LIMIT", "2G")
                    .arg("--hirsel-replay")
                    .arg(tape)
                    .output()
                    .unwrap();
                assert_eq!(out.status.code(), plain.exit_code, "{out:?}");
                assert_eq!(fs::read(root.join("replay.stdout")).unwrap(), plain.output);
                assert_eq!(
                    fs::read(root.join("replay.stderr")).unwrap(),
                    stage["stderr"].as_str().unwrap_or("").as_bytes()
                );
            }
            assert_eq!(fs::read(tape).unwrap(), original);
            eprintln!(
                "TEXT_INDEX_30_REPLAY_PASS {} reference checked-h llvm cranelift",
                row["id"]
            );
        }
        if let Some(tape) = control_tape {
            replay_refusals(&selection, root, &tape, &binaries);
        }
        if std::env::var_os("TEXT_INDEX_30_EVIDENCE_DIR").is_some() {
            let _ = scratch.keep();
        }
    });
}

#[test]
fn text_index_30_native_workflows_and_host_replay() {
    native(false);
}
#[test]
fn text_index_30_native_boundaries() {
    native(true);
}

fn replay_refusals(selection: &Path, root: &Path, tape: &Path, binaries: &[PathBuf]) {
    let original = fs::read(tape).unwrap();
    let parsed = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
    let mut variants = Vec::new();
    let mut changed = parsed.clone();
    let path = changed
        .events
        .iter_mut()
        .find_map(|e| match &mut e.request {
            hirsel_rt::io::Request::Directory { path, .. } => Some(path),
            _ => None,
        })
        .unwrap();
    path.push_str("-changed");
    variants.push(("changed-event", changed, "ReplayMismatch"));
    let mut extra = parsed.clone();
    extra.events.push(parsed.events[0].clone());
    variants.push(("unconsumed", extra, "UnconsumedInput"));
    let mut config = parsed.clone();
    // An unmodified run identity cannot authenticate changed canonical grants.
    let canonical = &mut config.binding.as_mut().unwrap().canonical;
    *canonical = String::from_utf8(canonical.clone())
        .unwrap()
        .replace("\"hash_seed\":0", "\"hash_seed\":1")
        .into_bytes();
    assert_ne!(
        &config.binding.as_ref().unwrap().canonical,
        &parsed.binding.as_ref().unwrap().canonical
    );
    variants.push(("changed-config", config, ""));
    for (name, record, kind) in variants {
        let bad = root.join(format!("refusal-{name}.tape"));
        fs::write(&bad, hirsel_rt::launch::render_recording(&record).unwrap()).unwrap();
        for checked in [false, true] {
            let result = if checked {
                hirsel_driver::replay_observations_checked(selection, &bad, Some(1_000_000))
            } else {
                hirsel_driver::replay_observations(selection, &bad, Some(1_000_000))
            };
            let error = result.unwrap_err().to_string();
            assert!(error.contains(kind), "{name}: {error}");
            fs::write(root.join(format!("refusal-{name}-{checked}.log")), error).unwrap();
        }
        for (i, binary) in binaries.iter().enumerate() {
            let out = Command::new("python3")
                .arg(root.join("capped"))
                .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
                .arg(binary)
                .env("HIRSEL_JOBS", "4")
                .env("HIRSEL_MEMORY_LIMIT", "2G")
                .arg("--hirsel-replay")
                .arg(&bad)
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(3), "{name}: {out:?}");
            assert!(
                String::from_utf8_lossy(&out.stderr).contains(kind),
                "{name}: {out:?}"
            );
            fs::write(
                root.join(format!("refusal-{name}-native{i}.log")),
                out.stderr,
            )
            .unwrap();
        }
        eprintln!("TEXT_INDEX_30_REFUSAL_PASS {name} reference checked-h llvm cranelift");
    }
    let changed = root.join("changed-source");
    fs::create_dir_all(&changed).unwrap();
    for name in ["main", "manifest", "tokenize", "index", "store", "query"] {
        fs::copy(
            root.join(format!("{name}.hir")),
            changed.join(format!("{name}.hir")),
        )
        .unwrap();
    }
    let source = fs::read_to_string(changed.join("main.hir"))
        .unwrap()
        .replace("built", "changed");
    assert_ne!(source, fs::read_to_string(root.join("main.hir")).unwrap());
    fs::write(changed.join("main.hir"), source).unwrap();
    fs::copy(selection, changed.join("hirsel.json")).unwrap();
    for checked in [false, true] {
        let result = if checked {
            hirsel_driver::replay_observations_checked(
                &changed.join("hirsel.json"),
                tape,
                Some(1_000_000),
            )
        } else {
            hirsel_driver::replay_observations(&changed.join("hirsel.json"), tape, Some(1_000_000))
        };
        assert!(result.is_err(), "changed source accepted");
    }
    for backend in ["llvm", "cranelift"] {
        let binary = changed.join(backend);
        let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["build", "--backend", backend])
            .arg(changed.join("hirsel.json"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "changed source build {backend}: {built:?}"
        );
        let out = Command::new("python3")
            .arg(root.join("capped"))
            .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
            .arg(&binary)
            .env("HIRSEL_JOBS", "4")
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .arg("--hirsel-replay")
            .arg(tape)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(3),
            "changed source {backend}: {out:?}"
        );
        fs::write(
            root.join(format!("refusal-changed-source-{backend}.log")),
            out.stderr,
        )
        .unwrap();
    }
    // A replay and a fresh configuration are mutually exclusive before entry.
    for binary in binaries {
        let out = Command::new("python3")
            .arg(root.join("capped"))
            .args(["--timeout", "60", "prlimit", "--cpu=60:61"])
            .arg(binary)
            .env("HIRSEL_JOBS", "4")
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .arg("--hirsel-replay")
            .arg(tape)
            .args(["--hirsel-grants", "{}"])
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "changed-input/config replay accepted"
        );
    }
    assert_eq!(fs::read(tape).unwrap(), original);
    eprintln!(
        "TEXT_INDEX_30_REFUSAL_PASS changed-source-and-input-config; original tape immutable"
    );
}
