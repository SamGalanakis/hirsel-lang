//! Full six-module entry with public simulation and checked host-replay adapters.
use hirsel_engine::*;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn package(root: &Path) -> PathBuf {
    for (name, source) in [
        ("main", include_str!("../../../../examples/pubsub/main.hir")),
        ("wire", include_str!("../../../../examples/pubsub/wire.hir")),
        ("hub", include_str!("../../../../examples/pubsub/hub.hir")),
        (
            "client",
            include_str!("../../../../examples/pubsub/client.hir"),
        ),
        ("flow", include_str!("../../../../examples/pubsub/flow.hir")),
        (
            "shutdown",
            include_str!("../../../../examples/pubsub/shutdown.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(include_str!("../../../../examples/pubsub/hirsel.json")).unwrap();
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
fn evidence() -> PathBuf {
    let path = std::env::var_os("PUBSUB_30_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    fs::create_dir_all(&path).unwrap();
    path
}
fn selected_cases(boundaries: bool) -> Vec<Value> {
    let input = if boundaries {
        include_str!("../support/pubsub_relay_30/boundaries.json")
    } else {
        include_str!("../support/pubsub_relay_30/cases.json")
    };
    let mut cases: Vec<Value> = serde_json::from_str(input).unwrap();
    if let Ok(filter) = std::env::var("PUBSUB_30_CASE") {
        cases.retain(|c| filter.split(',').any(|s| c["id"] == s));
    }
    assert!(!cases.is_empty(), "zero-test selection");
    cases
}
fn net(case: &Value) -> Value {
    let clients: Vec<_> = case["scripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| json!({"address":"0.0.0.0","port":0,"tls":"plain","steps":s}))
        .collect();
    json!({"provider":"net.script@2","rules":if case["id"]=="denied" {json!([])} else {json!([{"address":"/fixture/pubsub.sock","service":"0","operation":"unix_listen"}])},"peers":[],"listeners":[{"address":"/fixture/pubsub.sock","port":0,"clients":clients}]})
}
fn validate(case: &Value, output: &[u8], exit: Option<i32>) {
    assert_eq!(
        exit,
        case["exit"].as_i64().map(|v| v as i32),
        "{}: {}",
        case["id"],
        String::from_utf8_lossy(output)
    );
    let rows: Vec<Value> = String::from_utf8(output.to_vec())
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    if exit == Some(2) {
        assert_eq!(rows, vec![json!({"error":"usage_or_limit"})]);
        return;
    }
    if exit == Some(3) {
        assert_eq!(rows, vec![json!({"error":"io"})]);
        return;
    }
    assert_eq!(rows.first().unwrap(), &json!({"listening":true}));
    let report = rows.last().unwrap();
    assert_eq!(report["joined"], true, "{rows:?}");
    assert_eq!(report["admitted"], case["admitted"], "{rows:?}");
    assert_eq!(report["queued"], case["queued"], "{rows:?}");
    assert_eq!(report["written"], case["queued"], "{rows:?}");
    assert_eq!(report["disconnected"], 0, "{rows:?}");
    assert_eq!(report["credits_held"], 0);
    assert_eq!(report["reserved"], report["returned"]);
    assert!(report["peak_queue"].as_u64().unwrap() <= 4);
    assert_eq!(report["reason"], case["reason"]);
}
#[test]
fn pubsub_relay_30_reference_checked_full_app() {
    scripted(false)
}

#[test]
fn pubsub_relay_30_boundaries_reference_checked() {
    scripted(true)
}

fn scripted(boundaries: bool) {
    with_compiler_stack(|| {
        let scratch = tempfile::Builder::new()
            .prefix("pubsub-script-")
            .tempdir_in(evidence())
            .unwrap();
        let root = scratch.keep();
        let manifest = package(&root);
        let prepared = hirsel_driver::request_tir(&manifest, Verify::On, None)
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for case in selected_cases(boundaries) {
            let id = case["id"].as_str().unwrap();
            let mut simulation = SimulationConfig {
                args: serde_json::from_value(case["args"].clone()).unwrap(),
                net: Some(serde_json::from_value(net(&case)).unwrap()),
                ..Default::default()
            };
            simulation.grants.insert("MonotonicClock".into());
            simulation.projections.insert("monotonic_clock".into());
            for engine in ["reference", "checked-h"] {
                if std::env::var("PUBSUB_30_ENGINE").is_ok_and(|s| s != engine) {
                    continue;
                }
                eprintln!("PUBSUB_30_STARTED {id}/{engine}");
                // Fixed before execution; default corpus budget remains 1M.
                const STEPS: u64 = 10_000_000;
                let result = hirsel_driver::testing::observe_tir(
                    &prepared,
                    &run,
                    simulation.clone(),
                    hirsel_interp::Limits {
                        steps: Some(STEPS),
                        ..Default::default()
                    },
                    engine == "checked-h",
                )
                .unwrap();
                fs::write(
                    root.join(format!("{id}-{engine}.receipt")),
                    format!("{result:#?}"),
                )
                .unwrap();
                let observed = result
                    .observation
                    .as_ref()
                    .unwrap_or_else(|| panic!("{id}/{engine}: {result:?}"));
                assert!(observed.failures.is_empty(), "{id}/{engine}: {observed:?}");
                assert!(observed.blocked.is_none(), "{id}/{engine}: {observed:?}");
                validate(&case, &observed.output, observed.exit_code);
                let state = observed.final_state.as_ref().unwrap();
                assert!(state.stderr.is_empty(), "{state:?}");
                assert!(
                    state.handles.values().all(|grant| !matches!(
                        grant,
                        hirsel_interp::library::simulation::ObservedGrant::Stream { .. }
                            | hirsel_interp::library::simulation::ObservedGrant::Half { .. }
                            | hirsel_interp::library::simulation::ObservedGrant::Listener { .. }
                    )),
                    "live sockets after joined report: {state:?}"
                );
                eprintln!("PUBSUB_30_PASS {id}/{engine} {:?}", result.step_usage);
            }
        }
    });
}
// Historical unfiltered RE600 reproducer. Default coverage lives in cells.

fn original_cells(cases: &[Value]) -> Vec<String> {
    let mut names: Vec<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap().to_owned())
        .collect();
    names.extend(["host-success", "host-domain", "host-cancel"].map(str::to_owned));
    names
        .iter()
        .flat_map(|name| {
            ["llvm", "cranelift"]
                .into_iter()
                .flat_map(move |backend| [1, 2, 4].map(|width| format!("{name}-{backend}-{width}")))
        })
        .collect()
}

pub(super) fn native_cell(cell: &str) {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../support/pubsub_relay_30/cases.json")).unwrap();
    native(&[cell.to_owned()], &cases);
}

fn check_native_completion(retained: &Path, selected: &[String], cases: &[Value], success: bool) {
    assert!(success, "native child failed at {}", retained.display());
    let receipts: Vec<Value> =
        serde_json::from_slice(&fs::read(retained.join("receipts.json")).unwrap()).unwrap();
    let mut required = vec!["llvm-build".to_owned(), "cranelift-build".to_owned()];
    for cell in selected {
        required.push(cell.clone());
        required.extend(["llvm", "cranelift"].map(|backend| format!("{cell}-replay-{backend}")));
    }
    let actual: Vec<_> = receipts
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    let expected: std::collections::BTreeSet<_> = required.iter().collect();
    assert_eq!(
        actual.iter().collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    assert_eq!(actual.len(), required.len());
    assert!(
        receipts.iter().all(|row| row["passed"] == true),
        "{receipts:?}"
    );
    for stem in selected {
        let tape = fs::read(retained.join(format!("{stem}.tape"))).unwrap();
        assert!(!tape.is_empty(), "empty tape: {stem}");
        let _stdout = fs::read(retained.join(format!("{stem}.stdout")))
            .or_else(|_| fs::read(retained.join(stem).join("stdout")))
            .unwrap();
        let status = fs::read_to_string(retained.join(format!("{stem}.status")))
            .or_else(|_| fs::read_to_string(retained.join(stem).join("status")))
            .unwrap();
        let expected = if stem.starts_with("host-") {
            0
        } else {
            let id = stem.rsplitn(3, '-').nth(2).unwrap();
            cases.iter().find(|case| case["id"] == id).unwrap()["exit"]
                .as_i64()
                .unwrap() as i32
        };
        assert_eq!(status.trim().parse::<i32>().unwrap(), expected, "{stem}");
    }
}

fn replay_adapters(mut replay: impl FnMut(bool) -> Result<(), String>) -> Vec<String> {
    [false, true]
        .into_iter()
        .filter_map(|checked| replay(checked).err())
        .collect()
}

fn native(selected: &[String], cases: &[Value]) {
    assert!(!selected.is_empty(), "zero-test selection");
    let valid = original_cells(cases);
    let unique: std::collections::BTreeSet<_> = selected.iter().collect();
    assert_eq!(unique.len(), selected.len(), "duplicate selection");
    assert!(
        selected.iter().all(|cell| valid.contains(cell)),
        "unknown cell: {selected:?}"
    );
    let scratch = tempfile::Builder::new()
        .prefix("pubsub-native-")
        .tempdir_in(evidence())
        .unwrap();
    let _evidence = super::width_capacity_36::Evidence(scratch.path().to_owned());
    let manifest = package(scratch.path());
    fs::write(
        scratch.path().join("cases.json"),
        serde_json::to_vec(cases).unwrap(),
    )
    .unwrap();
    fs::write(
        scratch.path().join("verify.py"),
        include_str!("../support/pubsub_relay_30/verify.py"),
    )
    .unwrap();
    for (name, source) in [
        (
            "selection.py",
            include_str!("../support/pubsub_relay_30/selection.py"),
        ),
        (
            "host.py",
            include_str!("../support/pubsub_relay_30/host.py"),
        ),
        (
            "model.py",
            include_str!("../support/pubsub_relay_30/model.py"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
    ] {
        fs::write(scratch.path().join(name), source).unwrap();
    }
    let output = Command::new("python3")
        .arg(scratch.path().join("verify.py"))
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg(&manifest)
        .args(selected)
        .env_remove("PUBSUB_30_CELL")
        .env_remove("PUBSUB_30_CASE")
        .env_remove("PUBSUB_30_ENGINE")
        .output()
        .unwrap();
    fs::write(scratch.path().join("stdout"), &output.stdout).unwrap();
    fs::write(scratch.path().join("stderr"), &output.stderr).unwrap();
    let retained = scratch.keep();
    eprintln!(
        "native evidence {}: {}\n{}",
        retained.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    check_native_completion(&retained, selected, cases, output.status.success());
    let mut errors = Vec::new();
    // Every immutable native tape is consumed again by both public host adapters.
    with_compiler_stack(|| {
        let mut program = hirsel_driver::load_program(&manifest).unwrap();
        for stem in selected {
            let path = retained.join(format!("{stem}.tape"));
            let live_stdout = fs::read(retained.join(format!("{stem}.stdout")))
                .or_else(|_| fs::read(retained.join(stem).join("stdout")))
                .unwrap();
            let live_status = fs::read_to_string(retained.join(format!("{stem}.status")))
                .or_else(|_| fs::read_to_string(retained.join(stem).join("status")))
                .unwrap();
            let recording = hirsel_rt::host_launch::read_recording(&path).unwrap();
            recording.bound_run().unwrap();
            let placement = recording
                .binding
                .as_ref()
                .unwrap()
                .placement
                .as_ref()
                .unwrap();
            let width = stem.rsplit('-').next().unwrap().parse::<u64>().unwrap();
            let physical = stem.starts_with("host-");
            if physical {
                assert!(
                    placement.proves(width),
                    "missing host admission capacity: {stem}"
                );
            } else {
                assert!(matches!(&placement.admission,
                        hirsel_run_config::CapacityAdmission::NotPhysical {
                            requested: hirsel_run_config::Cores::Fixed(n),
                            reason: hirsel_run_config::CapacityExclusion::ScriptedWorld, ..
                        } if *n == width));
            }
            fs::write(
                path.with_extension("capacity.json"),
                serde_json::to_vec_pretty(&json!({"placement":placement,"physical":physical}))
                    .unwrap(),
            )
            .unwrap();
            let before = fs::read(&path).unwrap();
            errors.extend(replay_adapters(|checked| {
                let name = if checked { "checked-h" } else { "reference" };
                eprintln!("PUBSUB_30_STARTED {stem}/replay/{name}");
                let result = hirsel_driver::replay_loaded_observations(
                    &mut program,
                    &path,
                    Some(10_000_000),
                    checked,
                );
                let mut failure = None;
                let receipt = match result {
                    Ok(observed) => {
                        let expected_exit = live_status.trim().parse::<i32>().unwrap();
                        if observed.output != live_stdout
                            || observed.exit_code != Some(expected_exit)
                            || !observed.failures.is_empty()
                        {
                            failure = Some(format!("{stem}/{name}: output/exit mismatch"));
                        }
                        format!("{observed:#?}")
                    }
                    Err(error) => {
                        failure = Some(format!("{stem}/{name}: {error}"));
                        format!("INCOMPLETE: {error}")
                    }
                };
                fs::write(path.with_extension(format!("{name}-replay")), receipt).unwrap();
                failure.map_or(Ok(()), Err)
            }));
            assert_eq!(before, fs::read(&path).unwrap());
        }
    });
    assert!(
        output.status.success() && errors.is_empty(),
        "native/replay INCOMPLETE at {}: {errors:?}",
        retained.display()
    );
    for stem in selected {
        eprintln!("PUBSUB_30_REGISTRATION_PASS {stem} recording=1 native_replays=2 rh_replays=2");
    }
}

#[test]
fn pubsub_relay_30_real_unix_interpreter_and_replay() {
    let scratch = tempfile::Builder::new()
        .prefix("pubsub-host-")
        .tempdir_in(evidence())
        .unwrap();
    let root = scratch.keep();
    let manifest = package(&root);
    for (name, source) in [
        (
            "real.py",
            include_str!("../support/pubsub_relay_30/real.py"),
        ),
        (
            "host.py",
            include_str!("../support/pubsub_relay_30/host.py"),
        ),
        ("capped", include_str!("../../../../tools/capped")),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    let selection = std::env::var_os("PUBSUB_30_REPLAY_DIR").map(PathBuf::from);
    let path = if let Some(existing) = selection {
        existing
    } else {
        let destination = root.join("live");
        let output = Command::new("python3")
            .arg(root.join("real.py"))
            .arg(env!("CARGO_BIN_EXE_hirsel"))
            .arg(&manifest)
            .arg(&destination)
            .output()
            .unwrap();
        fs::write(root.join("host.stdout"), &output.stdout).unwrap();
        fs::write(root.join("host.stderr"), &output.stderr).unwrap();
        assert!(
            output.status.success(),
            "{}: {}\n{}",
            root.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        destination
    };
    let replay_manifest = path.join("../hirsel.json");
    with_compiler_stack(|| {
        let mut failures = Vec::new();
        for case in ["success", "domain", "cancel"] {
            let tape = path.join(format!("{case}.tape"));
            let bytes = fs::read(&tape).unwrap();
            let expected = fs::read(path.join(case).join("stdout")).unwrap();
            for checked in [false, true] {
                let name = if checked { "checked-h" } else { "reference" };
                let result = if checked {
                    hirsel_driver::replay_observations_checked(
                        &replay_manifest,
                        &tape,
                        Some(10_000_000),
                    )
                } else {
                    hirsel_driver::replay_observations(&replay_manifest, &tape, Some(10_000_000))
                };
                fs::write(
                    path.join(format!("{case}-{name}.replay")),
                    format!("{result:#?}"),
                )
                .unwrap();
                match result {
                    Ok(observed) => {
                        if observed.output != expected
                            || observed.exit_code != Some(0)
                            || !observed.failures.is_empty()
                        {
                            failures.push(format!("{case}/{name}: output/exit/failure mismatch"));
                        } else {
                            eprintln!("PUBSUB_30_REPLAY_PASS {case}/{name}");
                        }
                    }
                    Err(error) => failures.push(format!("{case}/{name}: {error}")),
                }
            }
            assert_eq!(fs::read(&tape).unwrap(), bytes);
        }
        // Mutations target a copy; all original tapes stay immutable.
        let original = path.join("success.tape");
        let parsed = hirsel_rt::host_launch::read_recording(&original).unwrap();
        let mut changed = parsed.clone();
        let written = changed
            .events
            .iter_mut()
            .find_map(|event| match &mut event.request {
                hirsel_rt::io::Request::Print { bytes, .. } => Some(bytes),
                _ => None,
            })
            .unwrap();
        written.push(b'!');
        let changed_input = path.join("changed-input.tape");
        fs::write(
            &changed_input,
            hirsel_rt::launch::render_recording(&changed).unwrap(),
        )
        .unwrap();
        let mut changed = parsed.clone();
        let binding = changed.binding.as_mut().unwrap();
        let mut config: Value = serde_json::from_slice(&binding.canonical).unwrap();
        config["args"][0] = "altered".into();
        binding.canonical = serde_json::to_vec(&config).unwrap();
        let changed_config = path.join("changed-config.tape");
        fs::write(
            &changed_config,
            hirsel_rt::launch::render_recording(&changed).unwrap(),
        )
        .unwrap();
        let mut changed = parsed.clone();
        changed.events.push(parsed.events[0].clone());
        let changed_tape = path.join("changed-tape.tape");
        fs::write(
            &changed_tape,
            hirsel_rt::launch::render_recording(&changed).unwrap(),
        )
        .unwrap();
        for bad in [&changed_input, &changed_config, &changed_tape] {
            for checked in [false, true] {
                let result = if checked {
                    hirsel_driver::replay_observations_checked(
                        &replay_manifest,
                        bad,
                        Some(10_000_000),
                    )
                } else {
                    hirsel_driver::replay_observations(&replay_manifest, bad, Some(10_000_000))
                };
                let error = result
                    .expect_err("mutated recording must refuse")
                    .to_string();
                fs::write(
                    bad.with_extension(if checked {
                        "checked-refusal"
                    } else {
                        "reference-refusal"
                    }),
                    &error,
                )
                .unwrap();
            }
        }
        assert_eq!(
            parsed,
            hirsel_rt::host_launch::read_recording(&original).unwrap()
        );
        assert!(failures.is_empty(), "{}: {failures:?}", path.display());
    });
}

#[test]
fn pubsub_relay_30_permit_drop_and_binding_controls() {
    with_compiler_stack(|| {
        let root = tempfile::Builder::new()
            .prefix("pubsub-credit-")
            .tempdir_in(evidence())
            .unwrap()
            .keep();
        let manifest = package(&root);
        let mut config: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        config["entry"]["name"] = "credits".into();
        config["sources"]
            .as_array_mut()
            .unwrap()
            .push(json!({"path":"credits.hir","module":[]}));
        fs::write(
            root.join("credits.hir"),
            include_str!("../support/pubsub_relay_30/credits.hir"),
        )
        .unwrap();
        fs::write(&manifest, config.to_string()).unwrap();
        let prepared = hirsel_driver::request_tir(&manifest, Verify::On, None)
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for checked in [false, true] {
            let result = hirsel_driver::testing::observe_tir(
                &prepared,
                &run,
                SimulationConfig::default(),
                hirsel_interp::Limits {
                    steps: Some(1_000_000),
                    ..Default::default()
                },
                checked,
            )
            .unwrap();
            fs::write(
                root.join(format!("credits-{checked}.receipt")),
                format!("{result:#?}"),
            )
            .unwrap();
            let observation = result.observation.expect("completed credit fixture");
            assert_eq!(observation.exit_code, Some(0));
            assert!(observation.failures.is_empty());
        }
        if let Some(path) = std::env::var_os("PUBSUB_30_REPLAY_DIR").map(PathBuf::from) {
            let original = path.join("success.tape");
            let mut changed = hirsel_rt::host_launch::read_recording(&original).unwrap();
            let canonical = &mut changed.binding.as_mut().unwrap().canonical;
            let text = String::from_utf8(canonical.clone()).unwrap();
            assert!(text.contains("\"args\":[\"serve\""));
            *canonical = text
                .replacen("\"args\":[\"serve\"", "\"args\":[\"altered\"", 1)
                .into_bytes();
            let bad = path.join("changed-args-canonical.tape");
            fs::write(&bad, hirsel_rt::launch::render_recording(&changed).unwrap()).unwrap();
            for checked in [false, true] {
                let result = if checked {
                    hirsel_driver::replay_observations_checked(
                        &path.join("../hirsel.json"),
                        &bad,
                        Some(10_000_000),
                    )
                } else {
                    hirsel_driver::replay_observations(
                        &path.join("../hirsel.json"),
                        &bad,
                        Some(10_000_000),
                    )
                };
                let reason = result
                    .expect_err("canonical Args mutation must refuse")
                    .to_string();
                assert!(!reason.contains("noncanonical"), "{reason}");
                fs::write(path.join(format!("changed-args-{checked}.refusal")), reason).unwrap();
            }
        }
    });
}

#[test]
fn pubsub_relay_30_registration_completion_contract() {
    let root = tempfile::tempdir().unwrap();
    let selected = vec!["denied-llvm-1".to_owned()];
    let cases = vec![json!({"id":"denied","exit":3})];
    let receipts = json!([
        {"id":"llvm-build","passed":true}, {"id":"cranelift-build","passed":true},
        {"id":"denied-llvm-1","passed":true},
        {"id":"denied-llvm-1-replay-llvm","passed":true},
        {"id":"denied-llvm-1-replay-cranelift","passed":true}
    ]);
    let receipt_path = root.path().join("receipts.json");
    fs::write(&receipt_path, receipts.to_string()).unwrap();
    fs::write(root.path().join("denied-llvm-1.tape"), b"fake tape").unwrap();
    fs::write(
        root.path().join("denied-llvm-1.stdout"),
        b"{\"error\":\"io\"}\n",
    )
    .unwrap();
    let status = root.path().join("denied-llvm-1.status");
    fs::write(&status, "3").unwrap();
    let check = || check_native_completion(root.path(), &selected, &cases, true);
    check();
    assert!(
        std::panic::catch_unwind(|| check_native_completion(root.path(), &selected, &cases, false))
            .is_err()
    );
    for code in ["124", "137", "0", "not-a-status"] {
        fs::write(&status, code).unwrap();
        assert!(std::panic::catch_unwind(check).is_err());
    }
    fs::write(&status, "3").unwrap();
    for index in 0..5 {
        let mut failed = receipts.clone();
        failed[index]["passed"] = false.into();
        fs::write(&receipt_path, failed.to_string()).unwrap();
        assert!(std::panic::catch_unwind(check).is_err());
        let mut missing = receipts.as_array().unwrap().clone();
        missing.remove(index);
        fs::write(&receipt_path, serde_json::to_vec(&missing).unwrap()).unwrap();
        assert!(std::panic::catch_unwind(check).is_err());
    }
    fs::write(&receipt_path, receipts.to_string()).unwrap();
    for suffix in ["tape", "stdout", "status"] {
        let path = root.path().join(format!("denied-llvm-1.{suffix}"));
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(std::panic::catch_unwind(check).is_err());
        fs::write(&path, bytes).unwrap();
    }
    let mut seen = Vec::new();
    let errors = replay_adapters(|checked| {
        seen.push(checked);
        Err(format!("fake adapter failure: {checked}"))
    });
    assert_eq!(seen, [false, true]);
    assert_eq!(errors.len(), 2);
    for invalid in ["unknown", "", "denied-llvm-3"] {
        assert!(std::panic::catch_unwind(|| native_cell(invalid)).is_err());
    }
}

#[test]
#[ignore = "manual historical RE600 diagnostic; bounded cells run by default"]
fn pubsub_relay_30_native_and_real_unix() {
    let cases = selected_cases(false);
    let cells = original_cells(&cases);
    let selected = std::env::var("PUBSUB_30_CELL")
        .map(|cell| vec![cell])
        .unwrap_or(cells);
    native(&selected, &cases);
}

#[test]
#[ignore = "explicit recovery of the ten historical native replay complements"]
fn historical_native_prefix_complements() {
    let cases = selected_cases(false);
    let mut selected = vec!["duplicate-llvm-1".to_string()];
    for name in ["no-subscriber", "one-receiver", "order-two"] {
        for width in [1, 2, 4] {
            selected.push(format!("{name}-llvm-{width}"));
        }
    }
    native(&selected, &cases);
}
