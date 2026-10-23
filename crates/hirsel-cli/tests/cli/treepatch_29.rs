//! Complete application entry/config/modules with independent target fixtures.
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

// Direct OS stderr retains receipts even when libtest captures passing tests.
macro_rules! receipt {
    ($($arg:tt)*) => {{
        use std::io::Write;
        writeln!(std::io::stderr().lock(), $($arg)*).unwrap();
    }};
}

fn data(value: &Value) -> Vec<u8> {
    if let Some(repeat) = value.as_object() {
        return repeat["repeat"]
            .as_str()
            .unwrap()
            .repeat(repeat["count"].as_u64().unwrap() as usize)
            .into_bytes();
    }
    if let Some(text) = value.as_str() {
        text.as_bytes().to_vec()
    } else {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_u64().unwrap() as u8)
            .collect()
    }
}

fn package(root: &Path) -> PathBuf {
    for (name, source) in [
        (
            "main",
            include_str!("../../../../examples/patchctl/main.hir"),
        ),
        (
            "model",
            include_str!("../../../../examples/patchctl/model.hir"),
        ),
        (
            "manifest",
            include_str!("../../../../examples/patchctl/manifest.hir"),
        ),
        (
            "plan",
            include_str!("../../../../examples/patchctl/plan.hir"),
        ),
        (
            "apply",
            include_str!("../../../../examples/patchctl/apply.hir"),
        ),
        (
            "publish",
            include_str!("../../../../examples/patchctl/publish.hir"),
        ),
        (
            "report",
            include_str!("../../../../examples/patchctl/report.hir"),
        ),
    ] {
        fs::write(root.join(format!("{name}.hir")), source).unwrap();
    }
    let mut selection: Value =
        serde_json::from_str(include_str!("../../../../examples/patchctl/hirsel.json")).unwrap();
    selection["dependencies"][0]["manifest"] = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .as_ref()
        .into();
    let path = root.join("hirsel.json");
    fs::write(&path, selection.to_string()).unwrap();
    path
}

#[test]
fn treepatch_29_scripted_reference_checked_h() {
    hirsel_engine::with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let selection = package(scratch.path());
        let mut loaded = hirsel_driver::load_program(&selection).unwrap();
        loaded.check_for_execution().unwrap();
        let config = hirsel_engine::RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../support/treepatch_29/cases.json")).unwrap();
        let mut completed = 0;
        let mut incomplete = 0;
        for case in cases {
            let id = case["id"].as_str().unwrap();
            if std::env::var("TREEPATCH_29_CASE")
                .is_ok_and(|filter| !filter.split(',').any(|x| x == id))
            {
                continue;
            }
            let mut world = hirsel_engine::SimulationConfig {
                args: serde_json::from_value(case["args"].clone()).unwrap(),
                ..Default::default()
            };
            world.mounts = [
                ("fixtures".into(), hirsel_run_schema::Access::Read),
                ("source".into(), hirsel_run_schema::Access::Read),
                ("output".into(), hirsel_run_schema::Access::ReadWrite),
            ]
            .into();
            world
                .directories
                .extend(["fixtures".into(), "source".into(), "output".into()]);
            world.files.insert("fixtures/change.json".into(), {
                let mut raw = case["envelope"].to_string();
                raw.push_str(&" ".repeat(case["envelope_padding"].as_u64().unwrap_or(0) as usize));
                raw.into_bytes()
            });
            for (name, data) in case["source"].as_object().unwrap() {
                world
                    .files
                    .insert(format!("source/{name}"), self::data(data));
                let mut parent = Path::new(name).parent();
                while let Some(p) = parent {
                    if p.as_os_str().is_empty() {
                        break;
                    }
                    world.directories.insert(format!("source/{}", p.display()));
                    parent = p.parent();
                }
            }
            if case["args"].as_array().unwrap().len() == 4 {
                world.projections.insert("monotonic_clock".into());
                world.grants.insert("MonotonicClock".into());
            }
            if case["deny_output"] == true {
                world
                    .mounts
                    .insert("output".into(), hirsel_run_schema::Access::Read);
            }
            if let Some(initial) = case["output_initial"].as_object() {
                for (name, value) in initial {
                    world.files.insert(format!("output/{name}"), data(value));
                }
            }
            if let Some(target) = case["target_input"].as_object() {
                world
                    .mounts
                    .insert("target".into(), hirsel_run_schema::Access::Read);
                world.directories.insert("target".into());
                for (name, value) in target {
                    world.files.insert(format!("target/{name}"), data(value));
                }
            }
            if let Some(fault) = case["fault"].as_str() {
                use hirsel_interp::library::simulation::Fault;
                world
                    .faults
                    .insert(fault.into(), [Fault::Canceled(1)].into());
            }
            if case["deny_source"] == true {
                world.mounts.remove("source");
            }
            let original = world.files.clone();
            let mut previous = None;
            for engine in ["reference", "checked-h"] {
                if std::env::var("TREEPATCH_29_SKIP")
                    .is_ok_and(|filter| filter.split(',').any(|x| x == format!("{id}/{engine}")))
                {
                    continue;
                }
                receipt!("TREEPATCH_29_STARTED {id} {engine}");
                let execution = if engine == "reference" {
                    hirsel_engine::run_observed_snapshot(&mut loaded.engine, &config, world.clone())
                } else {
                    hirsel_engine::run_observed_checked(&mut loaded.engine, &config, world.clone())
                }
                .unwrap();
                if matches!(&execution.outcome, Ok(hirsel_engine::RunOutcome::ToolingLimit(limit)) if limit.kind == "Steps" && limit.limit == 1_000_000)
                    && (case["boundary"] == true || id == "files-32")
                {
                    receipt!("TREEPATCH_29_INCOMPLETE {id} {engine}: {execution:?}");
                    incomplete += 1;
                    continue;
                }
                let observation = execution
                    .observation
                    .as_ref()
                    .unwrap_or_else(|| panic!("{id}/{engine}: {execution:?}"));
                assert_eq!(
                    observation.exit_code,
                    case["exit"].as_i64().map(|n| n as i32),
                    "{id}/{engine}: {execution:?}"
                );
                assert!(
                    observation.failures.is_empty() && observation.blocked.is_none(),
                    "{id}/{engine}: {execution:?}"
                );
                let state = observation.final_state.as_ref().unwrap();
                assert_eq!(
                    state.stderr,
                    case["stderr"].as_str().unwrap().as_bytes(),
                    "{id}/{engine}"
                );
                if case["stdout"].is_null() {
                    assert!(observation.output.is_empty(), "{id}/{engine}");
                } else {
                    assert_eq!(
                        serde_json::from_slice::<Value>(&observation.output).unwrap(),
                        case["stdout"],
                        "{id}/{engine}"
                    );
                }
                for (name, data) in &original {
                    assert_eq!(
                        state.trees[&0].1.get(name),
                        Some(data),
                        "source mutation {id}/{engine}"
                    );
                }
                let output: std::collections::BTreeMap<_, _> = state.trees[&0]
                    .1
                    .iter()
                    .filter_map(|(name, data)| {
                        name.strip_prefix("output/")
                            .map(|name| (name.to_owned(), data.clone()))
                    })
                    .collect();
                if case["exit"] == 0 && case["args"][0] == "tree-apply" {
                    let mut expected: std::collections::BTreeMap<String, Vec<u8>> = case["target"]
                        .as_object()
                        .unwrap()
                        .iter()
                        .map(|(k, v)| (k.clone(), data(v)))
                        .collect();
                    let completion: Value =
                        serde_json::from_slice(output.get(".patchctl-complete.json").unwrap())
                            .unwrap();
                    assert_eq!(completion["schema"], 1);
                    expected.insert(
                        ".patchctl-complete.json".into(),
                        output[".patchctl-complete.json"].clone(),
                    );
                    assert_eq!(output, expected, "{id}/{engine}");
                } else {
                    let expected: std::collections::BTreeMap<String, Vec<u8>> =
                        case["output_expected"]
                            .as_object()
                            .or_else(|| case["output_initial"].as_object())
                            .map(|initial| {
                                initial.iter().map(|(k, v)| (k.clone(), data(v))).collect()
                            })
                            .unwrap_or_default();
                    assert_eq!(output, expected, "{id}/{engine}");
                }
                if let Some(previous) = previous {
                    assert_eq!(*observation, previous, "{id}: complete observations");
                }
                previous = Some(observation.clone());
                completed += 1;
                receipt!("TREEPATCH_29_PASS {id} {engine}");
            }
        }
        assert!(completed > 0);
        receipt!("TREEPATCH_29_COMPLETED {completed} INCOMPLETE {incomplete}");
    });
}

#[test]
fn treepatch_29_host_native_and_replay() {
    if let Some(root) = std::env::var_os("TREEPATCH_29_AUDIT") {
        audit_tapes(&PathBuf::from(root));
        receipt!("TREEPATCH_29_TAPE_AUDIT_ONLY; no app engine run");
        return;
    }
    let scratch = tempfile::tempdir().unwrap();
    package(scratch.path());
    fs::write(
        scratch.path().join("cases.json"),
        include_str!("../support/treepatch_29/cases.json"),
    )
    .unwrap();
    fs::write(
        scratch.path().join("verify.py"),
        include_str!("../support/treepatch_29/verify.py"),
    )
    .unwrap();
    let result = Command::new("python3")
        .arg(scratch.path().join("verify.py"))
        .arg(env!("CARGO_BIN_EXE_hirsel"))
        .arg(scratch.path())
        .output()
        .unwrap();
    if !result.status.success() {
        let retained = scratch.keep();
        receipt!("TREEPATCH_29_RETAINED {}", retained.display());
        panic!(
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(report["completed"].as_u64().unwrap() > 0);
    receipt!("{}", String::from_utf8_lossy(&result.stderr));
    receipt!("TREEPATCH_29_HOST {}", json!(report));
    audit_tapes(&scratch.path().join("evidence"));
    if report["incomplete"].as_u64().unwrap() != 0 {
        receipt!("TREEPATCH_29_RETAINED {}", scratch.keep().display());
    }
}

// Existing public tape codec; preserve every event/identity/error/clock.
// Placement and admission timestamp alone are documented non-semantic metadata.
fn audit_tapes(root: &Path) {
    for backend in ["llvm", "cranelift"] {
        for case in ["mixed", "stale"] {
            if !root.join(format!("{backend}-{case}-w4.tape")).exists() {
                continue;
            }
            let mut reference = None;
            for width in [1, 2, 4] {
                let mut recording = hirsel_rt::host_launch::read_recording(
                    &root.join(format!("{backend}-{case}-w{width}.tape")),
                )
                .unwrap();
                let binding = recording.binding.as_mut().unwrap();
                binding.admitted_at = 0;
                binding.placement = None;
                if let Some(ref reference) = reference {
                    assert_eq!(
                        &recording, reference,
                        "complete recordings {backend}/{case}/w{width}"
                    );
                } else {
                    reference = Some(recording);
                }
            }
            receipt!("TREEPATCH_29_SEMANTIC_TAPES_PASS {backend}/{case} widths1/2/4");
        }
    }
    let input = root.join("llvm-mixed-w1.tape");
    if !input.exists() {
        return;
    }
    let original = hirsel_rt::host_launch::read_recording(&input).unwrap();
    let mut changed = original.clone();
    changed.arguments.push("changed".into());
    fs::write(
        root.join("refuse-input.tape"),
        hirsel_rt::launch::render_recording(&changed).unwrap(),
    )
    .unwrap();
    let mut changed = original;
    let binding = changed.binding.as_mut().unwrap();
    let canonical = String::from_utf8(binding.canonical.clone()).unwrap();
    assert!(canonical.contains("\"hash_seed\":0"));
    binding.canonical = canonical
        .replacen("\"hash_seed\":0", "\"hash_seed\":1", 1)
        .into_bytes();
    fs::write(
        root.join("refuse-config.tape"),
        hirsel_rt::launch::render_recording(&changed).unwrap(),
    )
    .unwrap();
}
