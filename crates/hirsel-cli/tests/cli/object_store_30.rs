//! Whole app entry/config/module acceptance. Oracles never execute Hirsel.
use hirsel_engine::*;
use hirsel_interp::observation::Effect;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn scratch(label: &str) -> tempfile::TempDir {
    let base = std::env::var_os("OBJECT_STORE_30_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(base)
        .unwrap()
}
fn package(root: &Path) -> PathBuf {
    for (name, data) in [
        (
            "main.hir",
            include_str!("../../../../examples/objstore/main.hir"),
        ),
        (
            "object.hir",
            include_str!("../../../../examples/objstore/object.hir"),
        ),
        (
            "store.hir",
            include_str!("../../../../examples/objstore/store.hir"),
        ),
        (
            "ingest.hir",
            include_str!("../../../../examples/objstore/ingest.hir"),
        ),
        (
            "tree.hir",
            include_str!("../../../../examples/objstore/tree.hir"),
        ),
        (
            "snapshot.hir",
            include_str!("../../../../examples/objstore/snapshot.hir"),
        ),
    ] {
        fs::write(root.join(name), data).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/objstore/hirsel.json")).unwrap();
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
fn unhex(value: &Value) -> Vec<u8> {
    let value = value.as_str().unwrap();
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn files(value: &Value) -> BTreeMap<String, Vec<u8>> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), unhex(v)))
        .collect()
}
fn oracle(root: &Path) -> Vec<Value> {
    let script = root.join("oracle.py");
    fs::write(
        &script,
        include_str!("../support/object_store_30/oracle.py"),
    )
    .unwrap();
    let out = Command::new("python3").arg(script).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    fs::write(root.join("cases.json"), &out.stdout).unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}
fn selected(case: &Value) -> bool {
    std::env::var("OBJECT_STORE_30_CASE")
        .map_or(true, |filter| filter.split(',').any(|id| case["id"] == id))
}
fn scripted(boundaries: bool) {
    with_compiler_stack(|| {
        let retained = scratch("object-store-30-script-").keep();
        let root = retained.as_path();
        let manifest = package(root);
        let cases = oracle(root);
        tool_seal(root);
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut passed = 0;
        let mut incomplete = 0;
        for case in cases
            .iter()
            .filter(|c| c["boundary"].as_bool().unwrap_or(false) == boundaries && selected(c))
        {
            let id = case["id"].as_str().unwrap();
            let mut world = SimulationConfig {
                args: case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().into())
                    .collect(),
                files: files(&case["files"]),
                ..Default::default()
            };
            world.mounts = [
                ("objects".into(), hirsel_run_schema::Access::ReadWrite),
                ("source".into(), hirsel_run_schema::Access::Read),
                (
                    "roots".into(),
                    if case["readonly"] == true {
                        hirsel_run_schema::Access::Read
                    } else {
                        hirsel_run_schema::Access::ReadWrite
                    },
                ),
            ]
            .into();
            world
                .directories
                .extend(["source".into(), "objects".into(), "roots".into()]);
            for name in world.files.keys().cloned().collect::<Vec<_>>() {
                let mut parent = Path::new(&name).parent();
                while let Some(p) = parent {
                    world.directories.insert(p.to_string_lossy().into());
                    parent = p.parent();
                }
            }
            if let Some(links) = case["links"].as_object() {
                for (k, v) in links {
                    world.links.insert(k.clone(), v.as_str().unwrap().into());
                }
            }
            if let Some(pending) = case["pending"].as_str() {
                world
                    .file_reads
                    .insert(pending.into(), hirsel_run_schema::FileReadScript::Pending);
            }
            if case["signals"] == true || case["signal_watch"] == true {
                world.projections.insert("signals".into());
                world.signals=Some(serde_json::from_value(serde_json::json!({"provider":"signals.script@1","kinds":["interrupt"],"script":if case["signals"]==true {vec!["interrupt"]} else {vec![]}})).unwrap());
            }
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                eprintln!("OBJECT_STORE_30_STARTED {id} {engine}");
                let bound = case["steps"].as_u64().unwrap();
                let execution = if engine == "reference" {
                    run_observed_snapshot_bounded(&mut loaded.engine, &config, world.clone(), bound)
                } else {
                    run_observed_checked_bounded(&mut loaded.engine, &config, world.clone(), bound)
                }
                .unwrap_or_else(|e| panic!("{id}/{engine}: {e}"));
                eprintln!(
                    "OBJECT_STORE_30_STEPS {id} {engine} {:?}",
                    execution.step_usage
                );
                let Some(observed) = execution.observation else {
                    assert!(
                        matches!(execution.outcome, Ok(RunOutcome::ToolingLimit(_))),
                        "{id}/{engine}: {:?}",
                        execution.outcome
                    );
                    fs::write(
                        root.join(format!("{id}-{engine}-INCOMPLETE")),
                        format!("{execution:?}"),
                    )
                    .unwrap();
                    incomplete += 1;
                    continue;
                };
                assert_eq!(
                    observed.exit_code,
                    case["exit"].as_i64().map(|n| n as i32),
                    "{id}/{engine}: {observed:?}"
                );
                assert_eq!(observed.output, unhex(&case["stdout"]), "{id}/{engine}");
                assert!(
                    observed.failures.is_empty()
                        && observed.blocked.is_none()
                        && observed.signaled.is_none(),
                    "{id}/{engine}: {observed:?}"
                );
                let state = observed.final_state.as_ref().unwrap();
                assert_eq!(
                    state.stderr,
                    unhex(&case["stderr"]),
                    "{id}/{engine}: {}",
                    String::from_utf8_lossy(&state.stderr)
                );
                let actual: BTreeMap<_, _> = state
                    .trees
                    .values()
                    .flat_map(|(_, files)| files.clone())
                    .collect();
                assert_eq!(actual, files(&case["after"]), "{id}/{engine}");
                let events: Vec<_> = observed
                    .effects
                    .iter()
                    .filter_map(|effect| match &effect.effect {
                        Effect::CapabilityConsumed(event) => Some(event.clone()),
                        _ => None,
                    })
                    .collect();
                if case["pending"].is_string() {
                    assert!(
                        events.iter().any(|e| e.operation == "file.read.pending"),
                        "{id}: worker never parked"
                    );
                    assert!(
                        !events.iter().any(|e| e.operation == "file.put_new"),
                        "{id}: canceled/error worker inserted"
                    );
                }
                if case["args"][0] == "snapshot" && case["exit"] == 0 {
                    let writes: Vec<_> = events
                        .iter()
                        .filter(|e| e.operation == "file.put_new")
                        .collect();
                    assert!(!writes.is_empty(), "root not published");
                    // Last mutation is root publication; all subsequent effects are output/cleanup.
                    assert!(
                        writes
                            .last()
                            .unwrap()
                            .input
                            .windows(4)
                            .any(|part| part == b"head")
                    );
                }
                let replay = RunConfiguration {
                    inputs: events,
                    ..config.clone()
                };
                let replayed = if engine == "reference" {
                    run_observed_snapshot_bounded(&mut loaded.engine, &replay, world.clone(), bound)
                } else {
                    run_observed_checked_bounded(&mut loaded.engine, &replay, world.clone(), bound)
                }
                .unwrap();
                assert_eq!(
                    replayed.observation.as_ref(),
                    Some(&observed),
                    "{id}/{engine}: exact scripted replay"
                );
                if let Some(old) = previous {
                    assert_eq!(old, observed, "{id}: complete ref/H observation");
                }
                previous = Some(observed);
                fs::write(
                    root.join(format!("{id}-{engine}-PASS")),
                    format!("bound={bound}\n"),
                )
                .unwrap();
                eprintln!("OBJECT_STORE_30_PASS {id} {engine}");
                passed += 1;
            }
        }
        assert!(passed + incomplete > 0, "zero selected workflow cells");
        eprintln!("OBJECT_STORE_30_TOTAL passed={passed} incomplete={incomplete}");
    });
}
#[test]
fn object_store_30_scripted_reference_checked() {
    scripted(false)
}
#[test]
#[ignore = "large exact boundary vectors; select separately under 2GiB"]
fn object_store_30_boundaries_reference_checked() {
    scripted(true)
}

#[test]
fn object_store_30_native_and_host_replay() {
    with_compiler_stack(|| {
        let scratch = scratch("object-store-30-native-");
        let retained = scratch.keep();
        let root = retained.as_path();
        let manifest = package(root);
        oracle(root);
        tool_seal(root);
        fs::write(
            root.join("native.py"),
            include_str!("../support/object_store_30/native.py"),
        )
        .unwrap();
        fs::write(
            root.join("capped"),
            include_str!("../../../../tools/capped"),
        )
        .unwrap();
        let out = Command::new("python3")
            .arg(root.join("native.py"))
            .arg("--hirsel")
            .arg(env!("CARGO_BIN_EXE_hirsel"))
            .arg("--root")
            .arg(root)
            .output()
            .unwrap();
        fs::write(root.join("driver.stdout"), &out.stdout).unwrap();
        fs::write(root.join("driver.stderr"), &out.stderr).unwrap();
        assert!(
            out.status.success(),
            "{}: {} {}",
            root.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        replay_receipts(root, &manifest);
    });
}

fn replay_receipts(root: &Path, manifest: &Path) {
    let receipts: Vec<Value> =
        serde_json::from_slice(&fs::read(root.join("receipts.json")).unwrap()).unwrap();
    assert!(!receipts.is_empty());
    let mut count = 0;
    for receipt in receipts {
        let id = receipt["id"].as_str().unwrap();
        if std::env::var("OBJECT_STORE_30_REPLAY_CASE")
            .is_ok_and(|filter| !filter.split(',').any(|v| v == id))
        {
            continue;
        }
        let tape = root.join(receipt["tape"].as_str().unwrap());
        let original = fs::read(&tape).unwrap();
        let bound = receipt["steps"].as_u64().unwrap();
        let mut previous = None;
        for engine in ["reference", "checked-h"] {
            if std::env::var("OBJECT_STORE_30_REPLAY_SKIP")
                .is_ok_and(|filter| filter.split(',').any(|v| v == format!("{id}/{engine}")))
            {
                continue;
            }
            let execution = if engine == "reference" {
                hirsel_driver::replay_observations(manifest, &tape, Some(bound))
            } else {
                hirsel_driver::replay_observations_checked(manifest, &tape, Some(bound))
            };
            match execution {
                Ok(observed) => {
                    assert_eq!(observed.output, unhex(&receipt["stdout"]), "{id}/{engine}");
                    assert_eq!(
                        observed.exit_code,
                        receipt["exit"].as_i64().map(|v| v as i32),
                        "{id}/{engine}"
                    );
                    assert!(observed.failures.is_empty());
                    if let Some(old) = previous {
                        assert!(observed.equivalent(&old), "{id}: full checked replay");
                    }
                    previous = Some(observed);
                    eprintln!("OBJECT_STORE_30_HOST_REPLAY_PASS {id} {engine}");
                }
                Err(error) => {
                    assert!(
                        ((id.ends_with("-verify") && bound == 1_000_000) || bound == 40_000_000)
                            && error.to_string().contains("no semantic observation"),
                        "{id}/{engine}: {error}"
                    );
                    eprintln!(
                        "OBJECT_STORE_30_HOST_REPLAY_INCOMPLETE {id} {engine} bound={bound}: {error}"
                    );
                }
            }
        }
        let recorded = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
        let stderr: Vec<u8> = recorded
            .events
            .iter()
            .filter_map(|event| match &event.request {
                hirsel_rt::io::Request::PrintStderr { bytes, .. } => Some(bytes.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(stderr, unhex(&receipt["stderr"]), "{id}");
        assert_eq!(fs::read(&tape).unwrap(), original);
        count += 1;
    }
    assert!(count > 0, "zero selected host replay cells");
    assert!(!root.join("world").exists());
}

#[test]
#[ignore = "replays only retained immutable tapes; select exact missing identities"]
fn object_store_30_retained_host_replay() {
    with_compiler_stack(|| {
        let root = PathBuf::from(std::env::var_os("OBJECT_STORE_30_REPLAY_ROOT").unwrap());
        replay_receipts(&root, &root.join("hirsel.json"));
    });
}

#[test]
#[ignore = "audits retained immutable tapes with original providers unavailable"]
fn object_store_30_tape_controls() {
    with_compiler_stack(|| {
        let root = PathBuf::from(std::env::var_os("OBJECT_STORE_30_REPLAY_ROOT").unwrap());
        let seed = root.join("llvm-put-hello.tape");
        let original = fs::read(&seed).unwrap();
        let record = hirsel_rt::launch::parse_recording_bytes(&original).unwrap();
        let audit = scratch("object-store-30-controls-").keep();
        let mut cases = Vec::new();
        let mut changed = record.clone();
        let event = changed
            .events
            .iter_mut()
            .find(|e| matches!(e.request, hirsel_rt::io::Request::FileRead { .. }))
            .unwrap();
        if let hirsel_rt::io::Request::FileRead { max, .. } = &mut event.request {
            *max += 1;
        }
        cases.push((
            "input",
            hirsel_rt::launch::render_recording(&changed)
                .unwrap()
                .into_bytes(),
        ));
        let mut changed = record.clone();
        let mut config = changed.bound_run().unwrap().record().clone();
        config.io.as_mut().unwrap().stdout = None;
        changed.binding.as_mut().unwrap().canonical =
            config.validated().unwrap().canonical().to_vec();
        cases.push((
            "config",
            hirsel_rt::launch::render_recording(&changed)
                .unwrap()
                .into_bytes(),
        ));
        let mut changed = original.clone();
        *changed.last_mut().unwrap() ^= 1;
        cases.push(("tape", changed));
        let mut changed = record.clone();
        changed.events.pop();
        cases.push((
            "missing",
            hirsel_rt::launch::render_recording(&changed)
                .unwrap()
                .into_bytes(),
        ));
        for (label, bytes) in cases {
            let tape = audit.join(format!("{label}.tape"));
            fs::write(&tape, bytes).unwrap();
            for engine in ["reference", "checked-h", "llvm", "cranelift"] {
                if engine == "reference" || engine == "checked-h" {
                    let out = if engine == "reference" {
                        hirsel_driver::replay_observations(
                            &root.join("hirsel.json"),
                            &tape,
                            Some(1_000_000),
                        )
                    } else {
                        hirsel_driver::replay_observations_checked(
                            &root.join("hirsel.json"),
                            &tape,
                            Some(1_000_000),
                        )
                    };
                    let error = out.expect_err("changed tape/config/input must refuse");
                    fs::write(
                        audit.join(format!("{label}-{engine}.error")),
                        error.to_string(),
                    )
                    .unwrap();
                } else {
                    let out = Command::new(root.join(format!("app-{engine}")))
                        .arg("--hirsel-replay")
                        .arg(&tape)
                        .output()
                        .unwrap();
                    assert_eq!(out.status.code(), Some(3), "{label}/{engine}: {out:?}");
                    assert!(out.stdout.is_empty());
                    fs::write(audit.join(format!("{label}-{engine}.stderr")), out.stderr).unwrap();
                }
                eprintln!("OBJECT_STORE_30_CONTROL_PASS {label} {engine}");
            }
        }
        let changed_dir = audit.join("changed-source");
        fs::create_dir(&changed_dir).unwrap();
        let changed_manifest = package(&changed_dir);
        for engine in ["reference", "checked-h"] {
            let check = if engine == "reference" {
                hirsel_driver::replay_observations(&changed_manifest, &seed, Some(1_000_000))
            } else {
                hirsel_driver::replay_observations_checked(
                    &changed_manifest,
                    &seed,
                    Some(1_000_000),
                )
            };
            let error = check.expect_err("source revision must bind before entry");
            assert!(
                error.to_string().to_lowercase().contains("program"),
                "{error}"
            );
            fs::write(
                audit.join(format!("source-{engine}.error")),
                error.to_string(),
            )
            .unwrap();
            eprintln!("OBJECT_STORE_30_CONTROL_PASS source {engine}");
        }
        if let Some(directory) = std::env::var_os("OBJECT_STORE_30_OTHER_BINARIES") {
            for engine in ["llvm", "cranelift"] {
                let out = Command::new(PathBuf::from(&directory).join(format!("app-{engine}")))
                    .arg("--hirsel-replay")
                    .arg(&seed)
                    .output()
                    .unwrap();
                assert_eq!(out.status.code(), Some(3), "source/{engine}: {out:?}");
                assert!(out.stdout.is_empty());
                assert!(
                    String::from_utf8_lossy(&out.stderr)
                        .to_lowercase()
                        .contains("program"),
                    "{out:?}"
                );
                fs::write(audit.join(format!("source-{engine}.stderr")), out.stderr).unwrap();
                eprintln!("OBJECT_STORE_30_CONTROL_PASS source {engine}");
            }
        }
        assert_eq!(fs::read(seed).unwrap(), original);
        assert!(!root.join("world").exists());
    });
}

fn tool_seal(root: &Path) {
    use sha2::{Digest, Sha256};
    fn visit(path: &Path, hashes: &mut BTreeMap<String, String>) {
        if path.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(&entry.unwrap().path(), hashes);
            }
        } else if path.is_file() {
            hashes.insert(
                path.canonicalize().unwrap().to_string_lossy().into(),
                format!("{:x}", Sha256::digest(fs::read(path).unwrap())),
            );
        }
    }
    let mut hashes = BTreeMap::new();
    for key in [
        "HIRSEL_PRODUCTS",
        "HIRSEL_RUNTIME_ARCHIVES",
        "LLVM_SYS_231_PREFIX",
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
    fs::write(
        root.join("consumer-tools.json"),
        serde_json::to_vec_pretty(&hashes).unwrap(),
    )
    .unwrap();
}
