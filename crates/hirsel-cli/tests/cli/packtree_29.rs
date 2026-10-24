//! Full shipped manifest/entry/module workflows with independent TAR and digest vectors.
use hirsel_engine::{RunConfiguration, SimulationConfig};
use hirsel_interp::{library::simulation::Fault, observation::Effect};
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn package(root: &std::path::Path) -> PathBuf {
    for (name, source) in [
        (
            "main",
            include_str!("../../../../examples/packtree/main.hir"),
        ),
        (
            "catalog",
            include_str!("../../../../examples/packtree/catalog.hir"),
        ),
        (
            "pack",
            include_str!("../../../../examples/packtree/pack.hir"),
        ),
        (
            "unpack",
            include_str!("../../../../examples/packtree/unpack.hir"),
        ),
        (
            "manifest",
            include_str!("../../../../examples/packtree/manifest.hir"),
        ),
        (
            "jobs",
            include_str!("../../../../examples/packtree/jobs.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/packtree/hirsel.json")).unwrap();
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
fn scratch(label: &str) -> tempfile::TempDir {
    let root = std::env::var_os("PACKTREE_29_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(root)
        .unwrap()
}
fn scripted(caps: bool) {
    hirsel_engine::with_compiler_stack(|| {
        let tmp = scratch("packtree-script-");
        let selection = package(tmp.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap_or_else(|e| panic!("{e}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let rows: Vec<Value> = if caps {
            for (name, source) in [
                (
                    "vectors.py",
                    include_str!("../support/packtree_29/vectors.py"),
                ),
                (
                    "oracle.py",
                    include_str!("../support/packtree_29/oracle.py"),
                ),
            ] {
                fs::write(tmp.path().join(name), source).unwrap();
            }
            let generated = Command::new("python3")
                .env("PYTHONDONTWRITEBYTECODE", "1")
                .arg(tmp.path().join("vectors.py"))
                .output()
                .unwrap();
            assert!(
                generated.status.success(),
                "{}",
                String::from_utf8_lossy(&generated.stderr)
            );
            serde_json::from_slice(&generated.stdout).unwrap()
        } else {
            serde_json::from_str(include_str!("../support/packtree_29/cases.json")).unwrap()
        };
        let twins: Vec<Value> =
            serde_json::from_str(include_str!("../support/packtree_29/reference-cases.json"))
                .unwrap();
        let mut completed = 0;
        for row in rows {
            let id = row["id"].as_str().unwrap();
            if std::env::var("PACKTREE_29_CASE").is_ok_and(|f| !f.split(',').any(|s| s == id)) {
                continue;
            }
            let row = twins
                .iter()
                .find(|twin| twin["full_id"] == row["id"])
                .map_or(row.clone(), |twin| twin["case"].clone());
            let id = row["id"].as_str().unwrap();
            let mut world = SimulationConfig {
                args: serde_json::from_value(row["args"].clone()).unwrap(),
                mounts: [
                    ("source".into(), hirsel_run_schema::Access::Read),
                    ("archives".into(), hirsel_run_schema::Access::ReadWrite),
                    ("target".into(), hirsel_run_schema::Access::ReadWrite),
                ]
                .into(),
                files: row["files"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(p, b)| (p.clone(), unhex(b.as_str().unwrap())))
                    .collect(),
                ..Default::default()
            };
            world
                .directories
                .extend(["source", "archives", "target"].map(str::to_owned));
            for dir in row["directories"].as_array().unwrap() {
                world.directories.insert(dir.as_str().unwrap().into());
            }
            for p in world.files.clone().keys() {
                let mut parent = std::path::Path::new(p).parent();
                while let Some(p) = parent {
                    world.directories.insert(p.to_str().unwrap().into());
                    parent = p.parent();
                }
            }
            for (p, dest) in row["links"].as_object().unwrap() {
                world.files.remove(p);
                world.links.insert(p.clone(), dest.as_str().unwrap().into());
            }
            if let Some(metadata) = row["metadata"].as_object() {
                for (p, size) in metadata {
                    world.metadata.insert(
                        p.clone(),
                        hirsel_interp::library::simulation::FileMetadata {
                            kind: 0,
                            size: size.as_u64().unwrap(),
                            modified_unix_ns: None,
                            permissions: 0o644,
                        },
                    );
                }
            }
            if row["denied"] == true {
                world.mounts.clear();
            }
            match row["fault"].as_str() {
                Some("canceled") => {
                    world
                        .faults
                        .insert("file.read".into(), [Fault::Canceled(0)].into());
                }
                Some(op) => {
                    world.faults.insert(
                        op.into(),
                        [Fault::Fault {
                            code: 77,
                            partial: 1,
                        }]
                        .into(),
                    );
                }
                _ => (),
            }
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                if std::env::var("PACKTREE_29_SKIP")
                    .is_ok_and(|f| f.split(',').any(|s| s == format!("{id}/{engine}")))
                {
                    continue;
                }
                eprintln!("PACKTREE_29_STARTED {id} {engine}");
                let execution = if engine == "reference" {
                    hirsel_engine::run_observed_snapshot(&mut loaded.engine, &config, world.clone())
                } else {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world.clone())
                }
                .unwrap();
                let observed = execution
                    .observation
                    .unwrap_or_else(|| panic!("{id}/{engine}: {:?}", execution.outcome));
                assert_eq!(
                    observed.exit_code,
                    Some(row["status"].as_i64().unwrap() as i32),
                    "{id}/{engine}: {observed:?}"
                );
                assert_eq!(
                    observed.output,
                    row["stdout"].as_str().unwrap().as_bytes(),
                    "{id}/{engine}"
                );
                let state = observed.final_state.as_ref().unwrap();
                assert_eq!(
                    state.stderr,
                    row["stderr"].as_str().unwrap().as_bytes(),
                    "{id}/{engine}"
                );
                assert!(
                    observed.failures.is_empty()
                        && observed.blocked.is_none()
                        && observed.signaled.is_none(),
                    "{id}/{engine}"
                );
                let files = &state.trees[&0].1;
                if row["incomplete"] == true {
                    assert!(files.contains_key("target/.packtree-incomplete"));
                    assert!(!files.contains_key("target/.packtree-manifest.json"));
                }
                for (p, b) in &world.files {
                    if p != "archives/out.tar" || row["status"] != 0 {
                        assert_eq!(files.get(p), Some(b), "{id}/{engine}/{p}");
                    }
                }
                if row["status"] == 0 && row["args"][0] == "unpack" {
                    assert!(!files.contains_key("target/.packtree-incomplete"));
                    assert_eq!(files["target/.packtree-manifest.json"], observed.output);
                    for entry in row["entries"].as_array().unwrap() {
                        if entry[1] == "file" {
                            assert_eq!(
                                files[&format!("target/{}", entry[0].as_str().unwrap())],
                                unhex(entry[2].as_str().unwrap())
                            );
                        }
                    }
                }
                let mut outstanding = 0usize;
                let mut maximum = 0;
                for event in &observed.effects {
                    if let Effect::CapabilityConsumed(e) = &event.effect {
                        match e.operation.as_str() {
                            "task.async" => {
                                outstanding += 1;
                                maximum = maximum.max(outstanding);
                            }
                            "task.await.complete" => outstanding = outstanding.saturating_sub(1),
                            "task.cancel_all" => outstanding = 0,
                            _ => (),
                        }
                    }
                }
                assert!(maximum <= 4 && outstanding == 0, "{id}/{engine}");
                if let Some(prior) = previous {
                    assert_eq!(observed, prior, "{id}: complete engine observation");
                }
                previous = Some(observed);
                completed += 1;
                eprintln!("PACKTREE_29_PASS {id} {engine} max_unjoined={maximum}");
            }
        }
        assert!(completed > 0);
        eprintln!("PACKTREE_29_COMPLETED {completed}");
    });
}
#[test]
fn packtree_29_reference_checked_workflows() {
    scripted(false)
}
#[test]
fn packtree_29_reference_checked_caps() {
    scripted(true)
}
fn compare_recordings(root: &std::path::Path) {
    let groups: std::collections::BTreeMap<String, Vec<String>> =
        serde_json::from_slice(&fs::read(root.join("qualification/semantic-groups.json")).unwrap())
            .unwrap();
    let mut compared = 0;
    for (workflow, tapes) in groups {
        assert_eq!(tapes.len(), 6);
        let mut previous = None;
        let mut trace = None;
        for path in tapes {
            let path = PathBuf::from(path);
            let mut recording = hirsel_rt::host_launch::read_recording(&path).unwrap();
            let binding = recording.binding.as_mut().unwrap();
            binding.admitted_at = 0;
            binding.placement = None;
            if let Some(prior) = &previous {
                assert_eq!(&recording, prior, "{workflow}: full event order/content");
            }
            previous = Some(recording);
            let bytes = fs::read(path.with_extension("trace")).unwrap();
            assert!(!bytes.is_empty());
            if let Some(prior) = &trace {
                assert_eq!(&bytes, prior, "{workflow}: canonical event trace");
            }
            trace = Some(bytes);
            compared += 1;
        }
    }
    assert_eq!(compared, 12);
    eprintln!(
        "PACKTREE_29_RECORDINGS_COMPARED {compared}: only admission timestamp and placement normalized"
    );
}
fn refusal_controls(root: &std::path::Path) {
    use sha2::{Digest, Sha256};
    let tape = root.join("evidence/llvm-pack-empty-w1.tape");
    let original = fs::read(&tape).unwrap();
    let recording = hirsel_rt::host_launch::read_recording(&tape).unwrap();
    let config: Value =
        serde_json::from_slice(&recording.binding.as_ref().unwrap().canonical).unwrap();
    let roots: Vec<PathBuf> = config["io"]["dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| PathBuf::from(d["host"].as_str().unwrap()))
        .collect();
    assert!(roots.iter().all(|p| !p.exists()));
    let out = root.join("binding-controls");
    fs::create_dir(&out).unwrap();
    let mut changed_input = recording.clone();
    changed_input.arguments.push("tampered".into());
    let inputs = out.join("input.tape");
    fs::write(
        &inputs,
        hirsel_rt::launch::render_recording(&changed_input).unwrap(),
    )
    .unwrap();
    let mut changed_config = recording.clone();
    let mut canonical = config;
    canonical["io"]["stdout"] = Value::Null;
    let config_path = out.join("changed-run.json");
    fs::write(&config_path, canonical.to_string()).unwrap();
    let validated = hirsel_engine::read_run_record(&config_path).unwrap();
    assert_ne!(validated.identity(), recording.run_identity);
    changed_config.binding.as_mut().unwrap().canonical = validated.canonical().to_vec();
    let configs = out.join("config.tape");
    fs::write(
        &configs,
        hirsel_rt::launch::render_recording(&changed_config).unwrap(),
    )
    .unwrap();
    let broken = out.join("truncated.tape");
    fs::write(&broken, &original[..original.len() - 1]).unwrap();
    let mut receipts = vec![];
    for (id, path, reason) in [
        ("input", inputs, "header disagrees"),
        ("config", configs, "recording.run_identity"),
        ("tape", broken, "recording"),
    ] {
        let bytes = fs::read(&path).unwrap();
        for engine in ["reference", "llvm", "cranelift"] {
            let mut command = Command::new("python3");
            command
                .arg(root.join("capped"))
                .args(["--timeout", "60", "prlimit", "--cpu=60:61"]);
            command
                .env("HIRSEL_MEMORY_LIMIT", "2G")
                .env("HIRSEL_JOBS", "4")
                .env("HIRSEL_DAEMON", "off");
            if engine == "reference" {
                command
                    .arg(env!("CARGO_BIN_EXE_hirsel"))
                    .arg("run")
                    .arg(root.join("hirsel.json"))
                    .arg("--replay");
            } else {
                command
                    .arg(root.join("evidence").join(engine))
                    .arg("--hirsel-replay");
            }
            command.arg(&path);
            let argv = format!("{command:?}");
            let observed = command.output().unwrap();
            fs::write(out.join(format!("{id}-{engine}.stdout")), &observed.stdout).unwrap();
            fs::write(out.join(format!("{id}-{engine}.stderr")), &observed.stderr).unwrap();
            assert_eq!(
                observed.status.code(),
                Some(3),
                "{id}/{engine}: {observed:?}"
            );
            assert!(observed.stdout.is_empty());
            assert!(
                String::from_utf8_lossy(&observed.stderr).contains(reason),
                "{id}/{engine}: {observed:?}"
            );
            assert!(roots.iter().all(|p| !p.exists()));
            assert_eq!(fs::read(&tape).unwrap(), original);
            assert_eq!(fs::read(&path).unwrap(), bytes);
            receipts.push(serde_json::json!({"id":id,"engine":engine,"argv":argv,"status":3,"reason":reason,"passed":true,"original_sha256":format!("{:x}",Sha256::digest(&original)),"tamper_sha256":format!("{:x}",Sha256::digest(&bytes))}));
            fs::write(
                out.join("receipts.json"),
                serde_json::to_vec_pretty(&receipts).unwrap(),
            )
            .unwrap();
            eprintln!("PACKTREE_29_BINDING_CONTROL_PASS {id} {engine}");
        }
    }
    assert_eq!(receipts.len(), 9);
}
// This worker owns one compiler session, but every replay constructs its own
// runtime and consumes its own bound tape. It runs alone in a capped child.
fn replay_batch(path: &std::path::Path) {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    struct Streams([OwnedFd; 2]);
    impl Drop for Streams {
        fn drop(&mut self) {
            std::io::stdout().flush().unwrap();
            std::io::stderr().flush().unwrap();
            for (index, saved) in self.0.iter().enumerate() {
                let fd = index as i32 + 1;
                assert_eq!(unsafe { libc::dup2(saved.as_raw_fd(), fd) }, fd);
            }
        }
    }
    hirsel_engine::with_compiler_stack(|| {
        hirsel_driver::session::enable_resident(1);
        let requests: Vec<Value> = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(!requests.is_empty() && requests.len() <= 8);
        for request in requests {
            let result = PathBuf::from(request["result"].as_str().unwrap());
            let stdout = result.with_extension("stdout");
            let stderr = result.with_extension("stderr");
            let files = [
                fs::File::create(&stdout).unwrap(),
                fs::File::create(&stderr).unwrap(),
            ];
            std::io::stdout().flush().unwrap();
            std::io::stderr().flush().unwrap();
            let saved = Streams(std::array::from_fn(|index| {
                let fd = unsafe { libc::dup(index as i32 + 1) };
                assert!(fd >= 0);
                unsafe { OwnedFd::from_raw_fd(fd) }
            }));
            for (index, file) in files.iter().enumerate() {
                let fd = index as i32 + 1;
                assert_eq!(unsafe { libc::dup2(file.as_raw_fd(), fd) }, fd);
            }
            let outcome = hirsel_driver::run_path_on_host(
                std::path::Path::new(request["manifest"].as_str().unwrap()),
                hirsel_driver::HostLaunch::Replay(std::path::Path::new(
                    request["tape"].as_str().unwrap(),
                )),
                Some(1_000_000),
                hirsel_engine::Verify::On,
            );
            drop(saved);
            let outcome = outcome.unwrap_or_else(|e| panic!("{e}"));
            assert!(
                matches!(outcome, hirsel_run_schema::RunOutcome::Returned(_)),
                "{outcome:?}"
            );
            fs::write(
                result,
                serde_json::to_vec(&serde_json::json!({
                    "status": outcome.exit_code(),
                    "stdout": String::from_utf8(fs::read(stdout).unwrap()).unwrap(),
                    "stderr": String::from_utf8(fs::read(stderr).unwrap()).unwrap(),
                }))
                .unwrap(),
            )
            .unwrap();
        }
    });
}

#[test]
fn packtree_29_native_workflows_replay() {
    if let Some(requests) = std::env::var_os("PACKTREE_29_HOST_REPLAY_BATCH") {
        replay_batch(&PathBuf::from(requests));
        return;
    }
    if let Some(root) = std::env::var_os("PACKTREE_29_BINDING_CONTROLS") {
        refusal_controls(&PathBuf::from(root));
        return;
    }
    if let Some(root) = std::env::var_os("PACKTREE_29_AUDIT_RECORDINGS") {
        // Read-only follow-up for previously executed complete app recordings.
        // This audit is not credited as an engine run or an additional replay.
        compare_recordings(&PathBuf::from(root));
        return;
    }
    let tmp = scratch("packtree-native-");
    package(tmp.path());
    for (name, source) in [
        (
            "verify.py",
            include_str!("../support/packtree_29/verify.py"),
        ),
        (
            "qualify.py",
            include_str!("../support/packtree_29/qualify.py"),
        ),
        (
            "oracle.py",
            include_str!("../support/packtree_29/oracle.py"),
        ),
        (
            "cases.json",
            include_str!("../support/packtree_29/cases.json"),
        ),
        (
            "reference-cases.json",
            include_str!("../support/packtree_29/reference-cases.json"),
        ),
        (
            "host_replay.py",
            include_str!("../support/packtree_29/host_replay.py"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
        (
            "vectors.py",
            include_str!("../support/packtree_29/vectors.py"),
        ),
    ] {
        fs::write(tmp.path().join(name), source).unwrap();
    }
    let output = Command::new("python3")
        .env(
            "PACKTREE_29_HOST_REPLAYER",
            std::env::current_exe().unwrap(),
        )
        .arg(tmp.path().join("verify.py"))
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(tmp.path())
        .output()
        .unwrap();
    fs::write(tmp.path().join("stdout"), &output.stdout).unwrap();
    fs::write(tmp.path().join("stderr"), &output.stderr).unwrap();
    let qualified = if output.status.success() {
        Some(
            Command::new("python3")
                .env(
                    "PACKTREE_29_HOST_REPLAYER",
                    std::env::current_exe().unwrap(),
                )
                .arg("-B")
                .arg(tmp.path().join("qualify.py"))
                .arg("--root")
                .arg(tmp.path())
                .arg("--hirsel")
                .arg(env!("CARGO_BIN_EXE_hirsel"))
                .output()
                .unwrap(),
        )
    } else {
        None
    };
    if let Some(q) = &qualified {
        fs::write(tmp.path().join("qualification.stdout"), &q.stdout).unwrap();
        fs::write(tmp.path().join("qualification.stderr"), &q.stderr).unwrap();
        assert!(q.status.success(), "{}", String::from_utf8_lossy(&q.stderr));
        compare_recordings(tmp.path());
        refusal_controls(tmp.path());
        let twin = Command::new("python3")
            .env(
                "PACKTREE_29_HOST_REPLAYER",
                std::env::current_exe().unwrap(),
            )
            .arg("-B")
            .arg(tmp.path().join("qualify.py"))
            .arg("--reference-twin")
            .arg("--root")
            .arg(tmp.path())
            .arg("--hirsel")
            .arg(env!("CARGO_BIN_EXE_hirsel"))
            .output()
            .unwrap();
        fs::write(
            tmp.path().join("qualification-reference.stdout"),
            &twin.stdout,
        )
        .unwrap();
        fs::write(
            tmp.path().join("qualification-reference.stderr"),
            &twin.stderr,
        )
        .unwrap();
        assert!(
            twin.status.success(),
            "{}",
            String::from_utf8_lossy(&twin.stderr)
        );
    }
    let path = tmp.keep();
    assert!(
        output.status.success(),
        "{}: {} {}",
        path.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn packtree_29_bounded_target_listing_native_replay() {
    let tmp = scratch("packtree-directory-probe-");
    package(tmp.path());
    for (name, source) in [
        (
            "verify.py",
            include_str!("../support/packtree_29/verify.py"),
        ),
        (
            "oracle.py",
            include_str!("../support/packtree_29/oracle.py"),
        ),
        (
            "cases.json",
            include_str!("../support/packtree_29/cases.json"),
        ),
        (
            "reference-cases.json",
            include_str!("../support/packtree_29/reference-cases.json"),
        ),
        (
            "vectors.py",
            include_str!("../support/packtree_29/vectors.py"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
    ] {
        fs::write(tmp.path().join(name), source).unwrap();
    }
    let output = Command::new("python3")
        .arg(tmp.path().join("verify.py"))
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--root")
        .arg(tmp.path())
        .env("PACKTREE_29_CASE", "unpack-go,target-nonempty")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let receipt: Value = serde_json::from_str(stdout.lines().last().unwrap())
        .unwrap_or_else(|error| panic!("invalid packtree summary: {error}: {stdout}"));
    let selected = receipt["selected"].as_u64().unwrap();
    assert!(selected > 0);
    assert_eq!(receipt["passed"].as_u64(), Some(selected));
    assert_eq!(receipt["incomplete_cells"].as_u64(), Some(0));
    eprintln!("PACKTREE_DIRECTORY_PROBE {receipt}");
}
