//! The actual package entry and capability projections run through public adapters.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn package(root: &Path) -> PathBuf {
    for (name, source) in [
        ("main", include_str!("../../../../examples/mdsite/main.hir")),
        (
            "model",
            include_str!("../../../../examples/mdsite/model.hir"),
        ),
        (
            "catalog",
            include_str!("../../../../examples/mdsite/catalog.hir"),
        ),
        (
            "assets",
            include_str!("../../../../examples/mdsite/assets.hir"),
        ),
        (
            "render",
            include_str!("../../../../examples/mdsite/render.hir"),
        ),
        (
            "publish",
            include_str!("../../../../examples/mdsite/publish.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/mdsite/hirsel.json")).unwrap();
    manifest["dependencies"][0]["manifest"] = Path::new(env!("CARGO_MANIFEST_DIR"))
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
        serde_json::from_str(include_str!("../support/sitebuild_29/cases.json")).unwrap();
    if let Ok(filter) = std::env::var("SITEBUILD_29_CASE") {
        cases.retain(|c| filter.split(',').any(|id| c["id"] == id));
    }
    assert!(!cases.is_empty(), "zero selected sitebuild cells");
    cases
}
fn config(case: &Value) -> String {
    format!(
        "content = \"content\"\nassets = \"assets\"\noutput = \"{}\"\ntitle = \"Site & sheep\"\njobs = {}\n",
        case["output"].as_str().unwrap_or("site"),
        case["jobs"].as_u64().unwrap_or(4)
    )
}
fn bytes(value: &Value) -> Vec<u8> {
    if let Some(s) = value.as_str() {
        s.as_bytes().to_vec()
    } else {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_u64().unwrap() as u8)
            .collect()
    }
}
fn hash(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
fn verify_manifest(tree: &BTreeMap<String, Vec<u8>>, cfg: &str) {
    let manifest: Value = serde_json::from_slice(&tree["data/site/.mdsite.json"]).unwrap();
    assert_eq!(manifest["version"], 1);
    assert_eq!(manifest["config"], hash(cfg.as_bytes()));
    for (name, entry) in manifest["files"].as_object().unwrap() {
        assert_eq!(entry["output"], hash(&tree[&format!("data/site/{name}")]));
        if let Some(source) = name.strip_suffix(".html").filter(|s| s.ends_with(".md")) {
            assert_eq!(entry["input"], hash(&tree[&format!("data/{source}")]));
        }
    }
    let outputs = tree.keys().filter(|p| p.starts_with("data/site/")).count();
    assert_eq!(outputs, manifest["files"].as_object().unwrap().len() + 1);
}
fn scripted_cases(selected: Vec<Value>) {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let selection = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let cases = selected;
        let mut attempted = 0;
        let mut completed = 0;
        let mut incomplete = 0;
        for case in &cases {
            let id = case["id"].as_str().unwrap();
            let cfg = config(case);
            let mut world = hirsel_engine::SimulationConfig {
                args: vec![
                    case["command"].as_str().unwrap_or("build").into(),
                    "site.toml".into(),
                ],
                mounts: [(
                    "data".into(),
                    if case["read_only"] == true {
                        hirsel_run_schema::Access::Read
                    } else {
                        hirsel_run_schema::Access::ReadWrite
                    },
                )]
                .into(),
                ..Default::default()
            };
            if case["legacy"] == true {
                world.args.clear();
                world.projections.remove("tasks");
            }
            world
                .directories
                .extend(["data", "data/content"].map(str::to_owned));
            if case["missing_assets"] != true {
                world.directories.insert("data/assets".into());
            }
            world
                .files
                .insert("data/site.toml".into(), cfg.as_bytes().to_vec());
            for (name, value) in case["files"].as_object().unwrap() {
                let name = format!("data/{name}");
                let mut parent = Path::new(&name).parent();
                while let Some(p) = parent {
                    world.directories.insert(p.to_string_lossy().into());
                    parent = p.parent();
                }
                world.files.insert(name, bytes(value));
            }
            for directory in case["directories"].as_array().into_iter().flatten() {
                world
                    .directories
                    .insert(format!("data/{}", directory.as_str().unwrap()));
            }
            if case["cancel"] == true {
                world.faults.insert(
                    "file.open".into(),
                    [hirsel_interp::library::simulation::Fault::Canceled(0)].into(),
                );
            }
            if let Some(fault) = case["fault"].as_str() {
                use hirsel_interp::library::simulation::{Fault, FileError};
                world.faults.insert(
                    "file.write_all".into(),
                    [if fault == "cancel" {
                        Fault::Canceled(0)
                    } else {
                        Fault::File {
                            error: FileError::PermissionDenied,
                            partial: 0,
                        }
                    }]
                    .into(),
                );
            }
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                if let Ok(cells) = std::env::var("SITEBUILD_29_CELLS") {
                    if !cells
                        .split(',')
                        .any(|cell| cell == format!("{id}/{engine}"))
                    {
                        continue;
                    }
                }
                attempted += 1;
                eprintln!("SITEBUILD_29_STARTED {id} {engine}");
                let execution = if engine == "reference" {
                    hirsel_engine::run_observed_snapshot(&mut loaded.engine, &run, world.clone())
                } else {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &run, world.clone())
                }
                .unwrap();
                let Some(observation) = execution.observation.as_ref() else {
                    assert!(
                        matches!(
                            execution.outcome,
                            Ok(hirsel_engine::RunOutcome::ToolingLimit(ref limit)) if limit.kind == "Steps" && limit.limit == 1_000_000
                        ),
                        "{id}/{engine}: {execution:?}"
                    );
                    eprintln!(
                        "SITEBUILD_29_INCOMPLETE {id} {engine}: {:?}",
                        execution.outcome
                    );
                    assert!(
                        [
                            "nested-three",
                            "unsafe-markdown",
                            "commonmark",
                            "go-walk-tree",
                            "depth-8",
                            "items-64",
                            "page-65536",
                            "input-1048576",
                            "output-2097152",
                            "output-2097153"
                        ]
                        .contains(&id),
                        "unexpected tooling limit {id}/{engine}"
                    );
                    incomplete += 1;
                    continue;
                };
                assert_eq!(
                    observation.exit_code,
                    Some(case["exit"].as_i64().unwrap() as i32),
                    "{id}/{engine}: {execution:?}"
                );
                assert!(
                    observation.failures.is_empty()
                        && observation.blocked.is_none()
                        && observation.signaled.is_none(),
                    "{observation:?}"
                );
                let output = String::from_utf8(observation.output.clone()).unwrap();
                if let Some(expected) = case["stdout"].as_str() {
                    assert_eq!(output, expected, "{id}/{engine}");
                } else {
                    assert!(
                        output.contains(case["error"].as_str().unwrap()),
                        "{id}/{engine}: {output}"
                    );
                }
                let tree = &observation.final_state.as_ref().unwrap().trees[&0].1;
                if case["exit"] == 0 && case["command"] != "plan" {
                    if case["legacy"] != true {
                        verify_manifest(tree, &cfg);
                    }
                    for (name, expected) in case["outputs"].as_object().into_iter().flatten() {
                        assert_eq!(
                            tree[&format!("data/site/{name}")],
                            expected.as_str().unwrap().as_bytes(),
                            "{id}/{engine}/{name}"
                        );
                    }
                    for (name, expected) in case["hashes"].as_object().into_iter().flatten() {
                        assert_eq!(
                            hash(&tree[&format!("data/site/{name}")]),
                            expected.as_str().unwrap(),
                            "{id}/{engine}/{name}"
                        );
                    }
                } else {
                    assert!(
                        !tree.keys().any(|p| p.starts_with("data/site/")),
                        "{id}: unexpected publication {tree:?}"
                    );
                }
                if let Some(previous) = previous {
                    assert_eq!(observation, &previous, "{id}: full semantic event order");
                }
                previous = Some(observation.clone());
                completed += 1;
                eprintln!("SITEBUILD_29_PASS {id} {engine}");
            }
        }
        let selected = std::env::var("SITEBUILD_29_CELLS")
            .map_or(cases.len() * 2, |cells| cells.split(',').count());
        assert!(selected > 0);
        assert_eq!(attempted, selected);
        assert_eq!(completed + incomplete, attempted);
        eprintln!(
            "SITEBUILD_29_COUNTS attempted={attempted} observed={completed} passed={completed} incomplete={incomplete}"
        );
    });
}
#[test]
fn sitebuild_29_scripted_reference_checked() {
    scripted_cases(
        cases()
            .into_iter()
            .filter(|case| {
                ![
                    "jobs-1",
                    "cancel-atomic",
                    "failed-atomic",
                    "go-walk-tree",
                    "legacy",
                ]
                .contains(&case["id"].as_str().unwrap())
            })
            .collect(),
    );
}
#[test]
fn sitebuild_29_additional_reference_checked() {
    scripted_cases(
        cases()
            .into_iter()
            .filter(|case| {
                [
                    "jobs-1",
                    "cancel-atomic",
                    "failed-atomic",
                    "go-walk-tree",
                    "legacy",
                ]
                .contains(&case["id"].as_str().unwrap())
            })
            .collect(),
    );
}
#[test]
fn sitebuild_29_bounds_reference_checked() {
    let specs: Vec<Value> =
        serde_json::from_str(include_str!("../support/sitebuild_29/caps.json")).unwrap();
    let mut vectors = vec![];
    for spec in specs {
        let mut files = serde_json::Map::new();
        for file in spec["files"].as_array().unwrap() {
            files.insert(
                file["path"].as_str().unwrap().into(),
                file["fill"]
                    .as_str()
                    .unwrap()
                    .repeat(file["size"].as_u64().unwrap() as usize)
                    .into(),
            );
        }
        let mut case = json!({"id":spec["id"],"files":files,"exit":if spec["error"].is_null(){0}else{2},"hashes":spec["output_hashes"],"error":spec["error"]});
        if spec["error"].is_null() {
            let pages = files
                .keys()
                .filter(|name| name.starts_with("content/"))
                .count();
            case["stdout"] = format!("pages {pages}; write {}; skip 0\n", files.len() + 1).into();
        }
        vectors.push(case);
    }
    scripted_cases(vectors);
}
fn native_workflows(caps: bool) {
    let base = std::env::var_os("SITEBUILD_29_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    let scratch = tempfile::Builder::new()
        .prefix("sitebuild-29-")
        .tempdir_in(base)
        .unwrap();
    let root = scratch.path();
    package(root);
    fs::write(
        root.join("verify.py"),
        include_str!("../support/sitebuild_29/verify.py"),
    )
    .unwrap();
    fs::write(
        root.join("cases.json"),
        include_str!("../support/sitebuild_29/cases.json"),
    )
    .unwrap();
    fs::write(
        root.join("literals.json"),
        include_str!("../support/sitebuild_29/literals.json"),
    )
    .unwrap();
    fs::write(
        root.join("caps.json"),
        include_str!("../support/sitebuild_29/caps.json"),
    )
    .unwrap();
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    let mut command = Command::new("python3");
    if caps {
        command.env("SITEBUILD_29_CAPS", "1");
    }
    let output = command
        .arg(root.join("verify.py"))
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(root)
        .output()
        .unwrap();
    fs::write(root.join("stdout"), &output.stdout).unwrap();
    fs::write(root.join("stderr"), &output.stderr).unwrap();
    let retained = scratch.keep();
    assert!(
        output.status.success(),
        "{}: {}\n{}",
        retained.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(receipt["passed"].as_u64().unwrap() > 0);
    eprintln!("SITEBUILD_29_NATIVE {receipt}");
    if std::env::var_os("SITEBUILD_29_SKIP_CONTROLS").is_none() {
        recording_controls(&retained);
    }
}

#[test]
fn sitebuild_29_native_workflows() {
    native_workflows(false);
}
#[test]
fn sitebuild_29_native_bounds() {
    native_workflows(true);
}

// Mutate copies through existing recording/run-record codecs. No replay adapter.
fn recording_controls(root: &Path) {
    let mut candidates: Vec<_> = fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("record.tape"))
        .filter(|p| p.is_file())
        .collect();
    candidates.sort();
    let original = candidates
        .first()
        .expect("at least one real application recording");
    let original_bytes = fs::read(original).unwrap();
    let record = hirsel_rt::host_launch::read_recording(original).unwrap();
    let mut inputs = record.clone();
    inputs.arguments.push("changed-input".into());
    let mut config = record.clone();
    let mut canonical: Value =
        serde_json::from_slice(&config.binding.as_ref().unwrap().canonical).unwrap();
    canonical["args"] = json!(["plan", "changed.toml"]);
    let run_file = root.join("changed-run.json");
    fs::write(&run_file, canonical.to_string()).unwrap();
    config.binding.as_mut().unwrap().canonical = hirsel_engine::read_run_record(&run_file)
        .unwrap()
        .canonical()
        .to_vec();
    let mut changed_tape = original_bytes.clone();
    changed_tape.extend_from_slice(b"changed-tape\n");
    let variants = [
        (
            "changed-input",
            hirsel_rt::launch::render_recording(&inputs)
                .unwrap()
                .into_bytes(),
        ),
        (
            "changed-config",
            hirsel_rt::launch::render_recording(&config)
                .unwrap()
                .into_bytes(),
        ),
        ("changed-tape", changed_tape),
    ];
    let binary_dir = std::env::var_os("SITEBUILD_29_BINARY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.to_owned());
    for (id, data) in variants {
        let tape = root.join(format!("{id}.tape"));
        fs::write(&tape, &data).unwrap();
        for engine in ["reference", "llvm", "cranelift"] {
            let mut command = Command::new("python3");
            command
                .env("HIRSEL_MEMORY_LIMIT", "2G")
                .env("HIRSEL_JOBS", "4")
                .arg(root.join("capped"))
                .args(["--timeout", "60", "prlimit", "--cpu=60:61"]);
            if engine == "reference" {
                command
                    .arg(env!("CARGO_BIN_EXE_hirsel"))
                    .arg("run")
                    .arg(root.join("hirsel.json"))
                    .arg("--replay");
            } else {
                command.arg(binary_dir.join(engine)).arg("--hirsel-replay");
            }
            let output = command.arg(&tape).output().unwrap();
            fs::write(root.join(format!("{id}-{engine}.stderr")), &output.stderr).unwrap();
            assert_eq!(output.status.code(), Some(3), "{id}/{engine}: {output:?}");
            assert!(output.stdout.is_empty(), "{id}/{engine}");
            assert_eq!(fs::read(&tape).unwrap(), data);
            assert_eq!(fs::read(original).unwrap(), original_bytes);
            eprintln!("SITEBUILD_29_CONTROL_PASS {id} {engine} exit=3");
        }
    }
}
