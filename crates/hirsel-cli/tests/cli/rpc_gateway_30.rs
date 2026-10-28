//! The actual multi-module entry, not a model or alternate runtime adapter.
use hirsel_engine::{RunConfiguration, SimulationConfig, with_compiler_stack};
use hirsel_interp::observation::Effect;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const STEPS: u64 = 4_000_000;
fn package(root: &Path) -> PathBuf {
    let std = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    let rpc = root.join("localrpc");
    fs::create_dir_all(&rpc).unwrap();
    for (name, source) in [
        (
            "main",
            include_str!("../../../../examples/rpcgate/main.hir"),
        ),
        (
            "config",
            include_str!("../../../../examples/rpcgate/config.hir"),
        ),
        (
            "wire",
            include_str!("../../../../examples/rpcgate/wire.hir"),
        ),
        (
            "route",
            include_str!("../../../../examples/rpcgate/route.hir"),
        ),
        (
            "attempt",
            include_str!("../../../../examples/rpcgate/attempt.hir"),
        ),
        (
            "report",
            include_str!("../../../../examples/rpcgate/report.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    fs::write(
        rpc.join("main.hir"),
        include_str!("../../../../examples/localrpc/main.hir"),
    )
    .unwrap();
    fs::write(
        rpc.join("model.hir"),
        include_str!("../../../../examples/localrpc/model.hir"),
    )
    .unwrap();
    let mut dependency: Value =
        serde_json::from_str(include_str!("../../../../examples/localrpc/hirsel.json")).unwrap();
    dependency["dependencies"][0]["manifest"] = json!(std);
    fs::write(rpc.join("hirsel.json"), dependency.to_string()).unwrap();
    let mut app: Value =
        serde_json::from_str(include_str!("../../../../examples/rpcgate/hirsel.json")).unwrap();
    app["dependencies"][0]["manifest"] = json!(std);
    app["dependencies"][1]["manifest"] = json!(rpc.join("hirsel.json"));
    let manifest = root.join("hirsel.json");
    fs::write(&manifest, app.to_string()).unwrap();
    manifest
}

fn evidence(label: &str) -> PathBuf {
    let root = std::env::var_os("RPC_GATEWAY_30_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(root)
        .unwrap()
        .keep()
}

fn cases() -> Vec<Value> {
    let mut rows: Vec<Value> =
        serde_json::from_str(include_str!("../support/rpc_gateway_30/cases.json")).unwrap();
    rows.extend(
        serde_json::from_str::<Vec<Value>>(include_str!(
            "../support/rpc_gateway_30/controlled_cases.json"
        ))
        .unwrap(),
    );
    if let Ok(filter) = std::env::var("RPC_GATEWAY_30_CASE") {
        rows.retain(|row| filter.split(',').any(|id| row["id"] == id));
    }
    assert!(!rows.is_empty(), "empty test selection");
    rows
}

fn scripted(group: &str) {
    with_compiler_stack(|| {
        let root = evidence("rpcgate-script-");
        let manifest = package(&root);
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap();
        loaded.check_for_execution().unwrap();
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut failures = vec![];
        let mut completed = 0;
        for case in cases()
            .into_iter()
            .filter(|row| row["group"].as_str().unwrap_or("basic") == group)
        {
            let id = case["id"].as_str().unwrap();
            let steps = case["steps"].as_u64().unwrap_or(STEPS);
            let mut world = SimulationConfig {
                args: serde_json::from_value(case["args"].clone()).unwrap(),
                net: case
                    .get("net")
                    .map(|v| serde_json::from_value(v.clone()).unwrap()),
                ..Default::default()
            };
            world.projections.insert("monotonic_clock".into());
            world.grants.insert("MonotonicClock".into());
            world.mounts = [("config".into(), hirsel_run_schema::Access::Read)].into();
            world.directories.insert("config".into());
            world.files.insert(
                "config/routes.json".into(),
                case["manifest"].to_string().into_bytes(),
            );
            let mut reference = None;
            for checked in [false, true] {
                let engine = if checked { "checked-h" } else { "reference" };
                eprintln!("RPC_GATEWAY_30 STARTED {id} {engine} steps={steps}");
                let execution = if checked {
                    hirsel_engine::run_observed_checked_bounded(
                        &mut loaded.engine,
                        &run,
                        world.clone(),
                        steps,
                    )
                } else {
                    hirsel_engine::run_observed_snapshot_bounded(
                        &mut loaded.engine,
                        &run,
                        world.clone(),
                        steps,
                    )
                }
                .unwrap();
                fs::write(
                    root.join(format!("{id}-{engine}.txt")),
                    format!(
                        "outcome={:?} step_usage={:?}",
                        execution.outcome, execution.step_usage
                    ),
                )
                .unwrap();
                eprintln!(
                    "RPC_GATEWAY_30 STEP_USAGE {id} {engine} {:?}",
                    execution.step_usage
                );
                let Some(observation) = execution.observation else {
                    failures.push(format!("{id}/{engine} INCOMPLETE {:?}", execution.outcome));
                    continue;
                };
                let actual = json!({"exit":observation.exit_code,"stdout":String::from_utf8_lossy(&observation.output)});
                if actual != case["expected"]
                    || !observation.failures.is_empty()
                    || observation.blocked.is_some()
                {
                    failures.push(format!(
                        "{id}/{engine}: {actual} failures={:?} blocked={:?}",
                        observation.failures, observation.blocked
                    ));
                }
                let events: Vec<_> = observation
                    .effects
                    .iter()
                    .filter_map(|e| match &e.effect {
                        Effect::CapabilityConsumed(event) => Some(event.clone()),
                        _ => None,
                    })
                    .collect();
                let connects = events
                    .iter()
                    .filter(|e| e.operation == "net.unix_connect")
                    .count();
                if connects != case["connects"].as_u64().unwrap() as usize {
                    failures.push(format!("{id}/{engine}: connects={connects}"));
                }
                fs::write(
                    root.join(format!("{id}-{engine}.events.json")),
                    serde_json::to_vec(&events).unwrap(),
                )
                .unwrap();
                if let Some(previous) = &reference {
                    assert_eq!(previous, &observation, "full effect order {id}");
                }
                reference = Some(observation);
                completed += 1;
                eprintln!("RPC_GATEWAY_30 COMPLETED {id} {engine}");
            }
        }
        eprintln!(
            "RPC_GATEWAY_30 completed={completed} evidence={}",
            root.display()
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

fn native(group: &str, live: bool) {
    let root = evidence("rpcgate-native-");
    let manifest = package(&root);
    fs::write(
        root.join("cases.json"),
        serde_json::to_vec(
            &cases()
                .into_iter()
                .filter(|row| row["group"].as_str().unwrap_or("basic") == group)
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("verify.py"),
        include_str!("../support/rpc_gateway_30/verify.py"),
    )
    .unwrap();
    fs::write(
        root.join("capped"),
        include_str!("../../../../tools/capped"),
    )
    .unwrap();
    fs::write(
        root.join("admission.py"),
        include_str!("../support/rpc_gateway_30/admission.py"),
    )
    .unwrap();
    let script = if live { "live.py" } else { "verify.py" };
    fs::write(
        root.join("live.py"),
        include_str!("../support/rpc_gateway_30/live.py"),
    )
    .unwrap();
    let output = Command::new("python3")
        .env(
            "RPC_GATEWAY_30_LIVE_MODES",
            if group == "extra" {
                "unsafe"
            } else {
                "success,error,cancel"
            },
        )
        .arg(root.join(script))
        .arg("--hirsel")
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg("--manifest")
        .arg(&manifest)
        .output()
        .unwrap();
    fs::write(root.join("report.json"), &output.stdout).unwrap();
    eprintln!(
        "RPC_GATEWAY_30_NATIVE_REPORT {}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::write(root.join("verify.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "{}: {}",
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["completed"].as_u64().unwrap() > 0);
    let mut errors: Vec<String> = report
        .get("failures")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(Value::to_string)
        .collect();
    with_compiler_stack(|| {
        let prepared =
            hirsel_driver::request_tir(&manifest, hirsel_engine::Verify::On, None).unwrap();
        for path in report["tapes"].as_array().unwrap() {
            let tape = Path::new(path["path"].as_str().unwrap());
            let steps = path["steps"].as_u64().unwrap_or(STEPS);
            let plain =
                hirsel_driver::replay_prepared_observations(&prepared, tape, Some(steps), false);
            let checked =
                hirsel_driver::replay_prepared_observations(&prepared, tape, Some(steps), true);
            match (plain, checked) {
                (Ok(reference), Ok(checked)) => {
                    assert!(reference.equivalent(&checked));
                    assert_eq!(
                        json!({"exit":reference.exit_code,"stdout":String::from_utf8_lossy(&reference.output)}),
                        path["expected"]
                    );
                    eprintln!(
                        "RPC_GATEWAY_30_HOST_REPLAY_PASS {} reference checked-h",
                        tape.display()
                    );
                }
                (plain, checked) => {
                    let receipt = format!("reference={plain:?}\nchecked={checked:?}");
                    fs::write(tape.with_extension("host-replay.txt"), &receipt).unwrap();
                    errors.push(format!(
                        "{}: {}",
                        tape.display(),
                        receipt
                            .lines()
                            .map(|l| l.chars().take(300).collect::<String>())
                            .collect::<Vec<_>>()
                            .join("; ")
                    ));
                }
            }
        }
    });
    assert!(
        errors.is_empty(),
        "INCOMPLETE owner-routed replay cells: {}",
        errors.join("\n")
    );
}

#[test]
fn rpc_gateway_30_scripted_reference_checked() {
    scripted("basic")
}
#[test]
fn rpc_gateway_30_advanced_reference_checked() {
    scripted("advanced")
}
#[test]
fn rpc_gateway_30_native_scripted_and_replay() {
    native("basic", false)
}
#[test]
fn rpc_gateway_30_advanced_native_and_replay() {
    native("advanced", false)
}
#[test]
fn rpc_gateway_30_production_ordered_live_and_replay() {
    native("advanced", true)
}

#[test]
fn rpc_gateway_30_boundary_reference_checked() {
    scripted("boundary")
}
#[test]
fn rpc_gateway_30_boundary_native_and_replay() {
    native("boundary", false)
}

#[test]
fn rpc_gateway_30_admission_reference_checked() {
    scripted("admission")
}
#[test]
fn rpc_gateway_30_admission_native_and_replay() {
    native("admission", false)
}

#[test]
fn rpc_gateway_30_pending_config_read_parent_cancellation() {
    with_compiler_stack(|| {
        let root = evidence("rpcgate-pending-");
        let application = package(&root);
        let wrapper = root.join("parent");
        fs::create_dir(&wrapper).unwrap();
        fs::write(
            wrapper.join("main.hir"),
            include_str!("../support/rpc_gateway_30/pending.hir"),
        )
        .unwrap();
        let std = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = wrapper.join("hirsel.json");
        fs::write(&manifest,json!({"schema":1,"package":{"name":"rpcgate_parent","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":std,"package":{"name":"std","version":"1"},"expose":[]},{"manifest":application,"package":{"name":"rpcgate","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let mut loaded = hirsel_driver::load_program(&manifest).unwrap();
        loaded.check_for_execution().unwrap();
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut world = SimulationConfig {
            args: vec![
                "serve".into(),
                "routes.json".into(),
                "/rpcgate/listen".into(),
            ],
            ..Default::default()
        };
        world.grants.insert("MonotonicClock".into());
        world.projections.insert("monotonic_clock".into());
        world.mounts = [("config".into(), hirsel_run_schema::Access::Read)].into();
        world
            .files
            .insert("config/routes.json".into(), b"{}".to_vec());
        world.directories.insert("config".into());
        world.file_reads.insert(
            "config/routes.json".into(),
            hirsel_run_schema::FileReadScript::Pending,
        );
        let mut previous = None;
        for checked in [false, true] {
            let execution = if checked {
                hirsel_engine::run_observed_checked_bounded(
                    &mut loaded.engine,
                    &run,
                    world.clone(),
                    STEPS,
                )
            } else {
                hirsel_engine::run_observed_snapshot_bounded(
                    &mut loaded.engine,
                    &run,
                    world.clone(),
                    STEPS,
                )
            }
            .unwrap();
            fs::write(
                root.join(format!("pending-{checked}.txt")),
                format!("{execution:#?}"),
            )
            .unwrap();
            let observed = execution.observation.unwrap();
            assert_eq!(observed.output, b"parent-canceled\n");
            assert_eq!(observed.exit_code, Some(0));
            assert!(observed.failures.is_empty());
            let events: Vec<_> = observed
                .effects
                .iter()
                .filter_map(|e| match &e.effect {
                    Effect::CapabilityConsumed(c) => Some(c.operation.as_str()),
                    _ => None,
                })
                .collect();
            assert!(events.contains(&"file.read.pending"));
            assert!(events.contains(&"file.close"));
            assert!(!events.contains(&"io.net"));
            if let Some(previous) = previous {
                assert_eq!(observed, previous);
            }
            previous = Some(observed);
            eprintln!(
                "RPC_GATEWAY_30_PENDING_CONFIG_PASS checked={checked} steps={:?}",
                execution.step_usage
            );
        }
    });
}

#[test]
fn rpc_gateway_30_replay_refuses_changed_source_config_and_tape() {
    with_compiler_stack(|| {
        let root = evidence("rpcgate-refusal-");
        let (manifest, original, binaries) = if let Ok(tape) =
            std::env::var("RPC_GATEWAY_30_REFUSAL_TAPE")
        {
            let tape = PathBuf::from(tape);
            let original_root = tape.parent().unwrap().parent().unwrap();
            (
                original_root.join("hirsel.json"),
                tape.clone(),
                [original_root.join("llvm"), original_root.join("cranelift")],
            )
        } else {
            let manifest = package(&root);
            let config = root.join("config");
            fs::create_dir(&config).unwrap();
            fs::write(
                config.join("routes.json"),
                cases()[0]["manifest"].to_string(),
            )
            .unwrap();
            let grants = root.join("grants.json");
            fs::write(&grants,json!({"args":["check","routes.json"],"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"config","host":config,"access":"read"}]}}).to_string()).unwrap();
            let binaries = [root.join("llvm"), root.join("cranelift")];
            for (backend, binary) in ["llvm", "cranelift"].into_iter().zip(&binaries) {
                let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                    .args(["build", "--backend", backend])
                    .arg(&manifest)
                    .arg("-o")
                    .arg(binary)
                    .output()
                    .unwrap();
                assert!(built.status.success(), "{built:?}");
            }
            let tape = root.join("seed.tape");
            let recorded = Command::new(&binaries[0])
                .arg("--hirsel-record")
                .arg(&tape)
                .arg("--hirsel-grants")
                .arg(grants)
                .args(["check", "routes.json"])
                .output()
                .unwrap();
            assert!(recorded.status.success(), "{recorded:?}");
            fs::remove_file(config.join("routes.json")).unwrap();
            (manifest, tape, binaries)
        };
        let bytes = fs::read(&original).unwrap();
        let record = hirsel_rt::launch::parse_recording_bytes(&bytes).unwrap();
        for kind in ["config", "extra-event", "args"] {
            let mut changed = record.clone();
            match kind {
                "config" => {
                    // Host reads are explicit recorded inputs (replay-child-artifact-29).
                    let event = changed.events.iter_mut().find(|e| {
                        matches!(e.request, hirsel_rt::io::Request::FileRead { .. })
                            && matches!(&e.completion,
                                hirsel_rt::io::Completion::FileInput(hirsel_rt::io::FileInput::Bytes(bytes))
                                if !bytes.is_empty())
                    }).unwrap();
                    event.completion = hirsel_rt::io::Completion::FileInput(
                        hirsel_rt::io::FileInput::Bytes(b"{}".to_vec()),
                    );
                }
                "extra-event" => changed.events.push(record.events[0].clone()),
                _ => changed.arguments.push("changed".into()),
            }
            let bad = root.join(format!("{kind}.tape"));
            fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap()).unwrap();
            assert!(hirsel_driver::replay_observations(&manifest, &bad, Some(STEPS)).is_err());
            assert!(
                hirsel_driver::replay_observations_checked(&manifest, &bad, Some(STEPS)).is_err()
            );
            for binary in &binaries {
                let refused = Command::new(binary)
                    .arg("--hirsel-replay")
                    .arg(&bad)
                    .output()
                    .unwrap();
                assert_eq!(refused.status.code(), Some(3), "{kind}: {refused:?}");
            }
            eprintln!("RPC_GATEWAY_30_REFUSAL_PASS {kind} all4");
        }
        // Bind the original immutable tape against a changed application revision.
        let changed_root = root.join("changed");
        fs::create_dir(&changed_root).unwrap();
        let original_root = manifest.parent().unwrap();
        let mut selected: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        for source in selected["sources"].as_array().unwrap() {
            let name = source["path"].as_str().unwrap();
            let text = fs::read_to_string(original_root.join(name))
                .unwrap()
                .replace("MAX_BYTES: u64 = 4096", "MAX_BYTES: u64 = 4095");
            fs::write(changed_root.join(name), text).unwrap();
        }
        for dep in selected["dependencies"].as_array_mut().unwrap() {
            let path = original_root
                .join(dep["manifest"].as_str().unwrap())
                .canonicalize()
                .unwrap();
            dep["manifest"] = json!(path);
        }
        let changed_manifest = changed_root.join("hirsel.json");
        fs::write(&changed_manifest, selected.to_string()).unwrap();
        assert!(
            hirsel_driver::replay_observations(&changed_manifest, &original, Some(STEPS)).is_err()
        );
        assert!(
            hirsel_driver::replay_observations_checked(&changed_manifest, &original, Some(STEPS))
                .is_err()
        );
        for backend in ["llvm", "cranelift"] {
            let binary = changed_root.join(backend);
            let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["build", "--backend", backend])
                .arg(&changed_manifest)
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert!(built.status.success(), "{built:?}");
            let refused = Command::new(binary)
                .arg("--hirsel-replay")
                .arg(&original)
                .output()
                .unwrap();
            assert_eq!(refused.status.code(), Some(3), "{refused:?}");
        }
        assert_eq!(fs::read(&original).unwrap(), bytes);
        eprintln!("RPC_GATEWAY_30_REFUSAL_PASS source all4");
    });
}

#[test]
fn rpc_gateway_30_non_idempotent_action_once_live_and_replay() {
    native("extra", true)
}

#[test]
fn rpc_gateway_30_disconnect_during_observed_backoff() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let root = evidence("rpcgate-backoff-");
    let manifest = package(&root);
    let sockets = tempfile::Builder::new()
        .prefix("rg-backoff-")
        .tempdir()
        .unwrap();
    let mut binaries = vec![];
    for backend in ["llvm", "cranelift"] {
        let binary = root.join(backend);
        if let Ok(prior) = std::env::var("RPC_GATEWAY_30_BINARIES") {
            fs::copy(Path::new(&prior).join(backend), &binary).unwrap();
        } else {
            let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["build", "--backend", backend])
                .arg(&manifest)
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert!(built.status.success(), "{built:?}");
        }
        binaries.push(binary);
    }
    for binary in &binaries {
        let backend = binary.file_name().unwrap().to_str().unwrap();
        let cell = root.join(format!("{backend}-backoff"));
        fs::create_dir(&cell).unwrap();
        let socket = sockets.path().join(format!("{backend}-in"));
        let upstream = sockets.path().join(format!("{backend}-up"));
        let peer = UnixListener::bind(&upstream).unwrap();
        let config = cell.join("config");
        fs::create_dir(&config).unwrap();
        fs::write(config.join("routes.json"),json!({"jobs":4,"clients":1,"request_ms":3000,"session_ms":15000,"routes":[{"method":"echo","socket":upstream,"idempotent":true}]}).to_string()).unwrap();
        let grants = cell.join("grants.json");
        let args = vec!["serve", "routes.json", socket.to_str().unwrap()];
        fs::write(&grants,json!({"args":args,"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","random":"random.splitmix64@1","monotonic_clock":"clock.monotonic.ns@1","dirs":[{"path":"config","host":config,"access":"read"}],"net":{"provider":"net.stream@1","rules":[{"address":socket,"service":"0","operation":"unix_listen"},{"address":upstream,"service":"0","operation":"unix_connect"}]}},"placement":{"cores":1,"commit":"shared","reorder":"off"}}).to_string()).unwrap();
        let tape = cell.join("run.tape");
        let mut child = Command::new(binary)
            .arg("--hirsel-record")
            .arg(&tape)
            .arg("--hirsel-grants")
            .arg(&grants)
            .args(&args)
            .env("HIRSEL_JOBS", "4")
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        reader.read_line(&mut ready).unwrap();
        assert_eq!(ready, "{\"listening\":true}\n");
        let mut client = UnixStream::connect(&socket).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let payload = br#"{"id":"x","method":"echo","args":"value"}"#;
        let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
        frame.extend(payload);
        client.write_all(&frame).unwrap();
        let (mut accepted, _) = peer.accept().unwrap();
        accepted
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut header = [0; 4];
        accepted.read_exact(&mut header).unwrap();
        let size = u32::from_be_bytes(header) as usize;
        assert!(size <= 2048);
        let mut payload = vec![0; size];
        accepted.read_exact(&mut payload).unwrap();
        let sent: Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(sent["id"], "c0r0a1");
        drop(accepted);
        // Public prefix parsing observes the third armed timer: session,
        // request, then retry backoff. No sleep guesses the cancellation phase.
        let stop = Instant::now() + Duration::from_secs(2);
        let mut witnessed = None;
        while Instant::now() < stop {
            if let Ok(bytes) = fs::read(&tape)
                && let Ok((record, _)) = hirsel_rt::tape_binary::parse_prefix(&bytes)
            {
                let timers: Vec<_> = record
                    .events
                    .iter()
                    .filter(|e| matches!(e.request, hirsel_rt::io::Request::SleepUntil { .. }))
                    .collect();
                if timers.len() >= 3 {
                    witnessed = Some(record);
                    break;
                }
            }
            std::thread::yield_now();
        }
        let witness = witnessed.expect("backoff timer was never observed");
        assert_eq!(
            witness
                .events
                .iter()
                .filter(|e| matches!(e.request, hirsel_rt::io::Request::UnixConnect { .. }))
                .count(),
            1,
            "retry already started"
        );
        fs::write(cell.join("backoff-prefix.txt"), format!("{witness:#?}")).unwrap();
        drop(client);
        let mut suffix = String::new();
        reader.read_to_string(&mut suffix).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let expected = "{\"listening\":true}\n{\"client\":0,\"request\":0,\"attempt\":1,\"status\":\"transport\"}\n{\"clients\":1,\"expired\":false}\n";
        assert_eq!(ready + &suffix, expected);
        peer.set_nonblocking(true).unwrap();
        assert_eq!(
            peer.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(peer);
        fs::remove_file(&upstream).unwrap();
        fs::remove_file(config.join("routes.json")).unwrap();
        assert!(!socket.exists());
        with_compiler_stack(|| {
            let ordinary =
                hirsel_driver::replay_observations(&manifest, &tape, Some(STEPS)).unwrap();
            let checked =
                hirsel_driver::replay_observations_checked(&manifest, &tape, Some(STEPS)).unwrap();
            assert!(ordinary.equivalent(&checked));
            assert_eq!(ordinary.output, expected.as_bytes());
        });
        for target in &binaries {
            let replay = Command::new(target)
                .arg("--hirsel-replay")
                .arg(&tape)
                .output()
                .unwrap();
            assert!(replay.status.success(), "{replay:?}");
            assert_eq!(replay.stdout, expected.as_bytes());
            assert!(replay.stderr.is_empty());
        }
        eprintln!(
            "RPC_GATEWAY_30_BACKOFF_PASS {backend} observed-timer disconnect joined no-second-connect all4replay"
        );
    }
}

#[test]
fn rpc_gateway_30_controlled_admission_reference_checked() {
    // Simulation's logical clock admits the complete input prefix before the
    // unanswered operations advance time. Original live cases stay unchanged.
    scripted("controlled-admission")
}
