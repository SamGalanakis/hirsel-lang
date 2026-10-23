//! Private engine differential checks run without exporting selectors from std.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::{Path, PathBuf};

pub(super) fn private_fixture(directory: &Path, _interpreter: bool) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let std_root = root.join("stdlib/std@1").canonicalize().unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(std_root.join("hirsel.json")).unwrap()).unwrap();
    // Check the unchanged regex owners as a private root, not every exported
    // std module. Keep exact source builders for their only text dependency.
    manifest["sources"]
        .as_array_mut()
        .unwrap()
        .retain(|source| {
            let module = source["module"].as_array().unwrap();
            module == &vec![serde_json::json!("regex")]
                || module == &vec![serde_json::json!("ascii")]
                || module
                    == &vec![
                        serde_json::json!("internal"),
                        serde_json::json!("substring"),
                    ]
        });
    for source in manifest["sources"].as_array_mut().unwrap() {
        source["path"] = std_root
            .join(source["path"].as_str().unwrap())
            .into_os_string()
            .into_string()
            .unwrap()
            .into();
    }
    let text = std::fs::read_to_string(std_root.join("text.hir")).unwrap();
    let builders = text.split("// Pads the text").next().unwrap();
    std::fs::write(directory.join("text.hir"), builders).unwrap();
    manifest["sources"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": directory.join("text.hir"), "module": ["text"]
        }));
    manifest["sources"].as_array_mut().unwrap().push(serde_json::json!({
        "path": root.join("crates/hirsel-driver/tests/fixtures/regex-capture-span-17.hir").canonicalize().unwrap(),
        "module": ["regex"]
    }));
    manifest["entry"] = serde_json::json!({"module": ["regex"], "name": "capture_exhaustive"});
    verify_trace_copies(&root, &std_root);
    let path = directory.join("hirsel.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    path
}

fn check_capture(backend: Option<native::Backend>, selected_entries: &[&str]) {
    with_compiler_stack(|| {
        let temp = tempfile::tempdir().unwrap();
        let path = private_fixture(temp.path(), false);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for &entry in selected_entries {
            eprintln!("capture span {entry} {backend:?}");
            manifest["entry"]["name"] = entry.into();
            std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            let outcome = match backend {
                None => run_path_with_step_budget(
                    &path,
                    SimulationConfig::default(),
                    Some(100_000_000),
                    Verify::On,
                    &mut Vec::new(),
                    &mut Vec::new(),
                )
                .unwrap(),
                Some(selected) => native::run_path_native_backend(
                    &path,
                    HostLaunch::empty(),
                    None,
                    selected,
                    native::Output::Streams {
                        stdout: &mut Vec::new(),
                        stderr: &mut Vec::new(),
                    },
                    Verify::On,
                )
                .unwrap()
                .unwrap(),
            };
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{entry} {backend:?}: {outcome:?}"
            );
        }
    });
}
// Reuse source registration and checked compiler products within a bounded
// shard. Each entry still checks with V on and starts a fresh simulation and
// interpreter, including its own unchanged 100M-step budget.
fn check_interpreter(selected_entries: &[&str]) {
    with_compiler_stack(|| {
        let temp = tempfile::tempdir().unwrap();
        let path = private_fixture(temp.path(), true);
        let mut loaded = COMPILER
            .with_verify(Verify::On)
            .load_program(&path)
            .unwrap();
        for &entry in selected_entries {
            eprintln!("capture span {entry} None");
            loaded.entry.0.name = entry.into();
            loaded.check_for_execution().unwrap();
            loaded.refuse_foreign_interpretation().unwrap();
            loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
                limits: hirsel_interp::Limits {
                    steps: Some(100_000_000),
                    ..Default::default()
                },
                ..Default::default()
            }));
            let outcome = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    entry: loaded.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap()
            .outcome
            .unwrap();
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{entry} None: {outcome:?}"
            );
        }
    });
}

// Independently registered cases let the existing measured-cost launcher
// distribute this matrix; no single test action runs all 42 entries.
macro_rules! interpreter_shard {
    ($name:ident, [$($entry:literal),+ $(,)?]) => {
        #[test]
        fn $name() {
            check_interpreter(&[$($entry),+]);
        }
    };
}
interpreter_shard!(
    capture_span_interpreter_00,
    [
        "capture_exhaustive_0",
        "capture_exhaustive_1",
        "capture_exhaustive_2",
        "capture_exhaustive_3",
        "capture_exhaustive_4",
        "capture_exhaustive_5",
    ]
);
interpreter_shard!(
    capture_span_interpreter_01,
    [
        "capture_exhaustive_6",
        "capture_exhaustive_7",
        "capture_exhaustive_8",
        "capture_exhaustive_9",
        "capture_exhaustive_10",
        "capture_exhaustive_11",
    ]
);
interpreter_shard!(
    capture_span_interpreter_02,
    [
        "capture_exhaustive_12",
        "capture_exhaustive_13",
        "capture_exhaustive_14",
        "capture_exhaustive_15",
        "capture_replacements_0_0",
        "capture_replacements_0_1",
    ]
);
interpreter_shard!(
    capture_span_interpreter_03,
    [
        "capture_replacements_0_2",
        "capture_replacements_1_0",
        "capture_replacements_1_1",
        "capture_replacements_1_2",
        "capture_replacements_2_0",
        "capture_replacements_2_1",
    ]
);
interpreter_shard!(
    capture_span_interpreter_04,
    [
        "capture_replacements_2_2",
        "capture_replacements_3_0",
        "capture_replacements_3_1",
        "capture_replacements_3_2",
        "capture_budgets",
        "capture_goldens",
    ]
);
interpreter_shard!(
    capture_span_interpreter_05,
    [
        "capture_retention",
        "capture_expand_partial",
        "capture_forced_limits",
        "capture_context_0",
        "capture_context_1",
        "capture_context_2",
    ]
);
interpreter_shard!(
    capture_span_interpreter_06,
    [
        "capture_context_3",
        "capture_context_4",
        "capture_context_5",
        "capture_context_6",
        "capture_context_7",
        "capture_context_8",
    ]
);
const NATIVE_ENTRIES: &[&str] = &[
    "capture_exhaustive",
    "capture_budgets",
    "capture_replacements",
    "capture_goldens",
    "capture_context_and_retention",
    "capture_expand_partial",
    "capture_forced_limits",
];
#[test]
fn capture_span_llvm() {
    check_capture(Some(native::Backend::Llvm), NATIVE_ENTRIES);
}
#[test]
fn capture_span_cranelift() {
    check_capture(Some(native::Backend::Cranelift), NATIVE_ENTRIES);
}

fn verify_trace_copies(root: &Path, std_root: &Path) {
    let fixture = std::fs::read_to_string(
        root.join("crates/hirsel-driver/tests/fixtures/regex-capture-span-17.hir"),
    )
    .unwrap();
    for (name, file, end) in [
        ("replace_pike", "regex.hir", Some("\npub fn replace_all(")),
        ("replace_accelerated", "regex_capture.hir", None),
    ] {
        let source = std::fs::read_to_string(std_root.join(file)).unwrap();
        let original = source.split_once(&format!("fn {name}[T](")).unwrap().1;
        let original = end.map_or(original, |end| original.split_once(end).unwrap().0);
        let traced = fixture
            .split_once(&format!("fn traced_{name}[T]("))
            .unwrap()
            .1;
        let traced = traced.split_once("\nfn ").map_or(traced, |(head, _)| head);
        let traced = traced
            .split_once("\npub fn ")
            .map_or(traced, |(head, _)| head);
        let traced = traced
            .replace(",\n  mut trace: List[u64]", "")
            .replace("        core@1/list.push(mut trace, span.start)\n", "")
            .replace("        core@1/list.push(mut trace, span.end)\n", "");
        fn compact(source: &str) -> String {
            source
                .lines()
                .map(|line| line.split("//").next().unwrap())
                .collect::<String>()
                .chars()
                .filter(|c| !c.is_whitespace() && *c != ';')
                .collect()
        }
        assert_eq!(
            compact(original),
            compact(&traced),
            "trace copy of {name} drifted"
        );
    }
}

// A bounded wrapper check is useful when the complete differential matrices
// have already run from frozen production sources outside this test harness.
#[test]
fn capture_span_wrapper_smoke() {
    for backend in [
        None,
        Some(native::Backend::Llvm),
        Some(native::Backend::Cranelift),
    ] {
        check_capture(backend, &["capture_forced_limits"]);
    }
}

// Keep this budget regression independently runnable from the aggregate sweep.
#[test]
fn capture_replacements_ambiguous_interpreter_budget() {
    check_capture(None, &["capture_replacements_1_0"]);
}

// Keep the original complete matrices above. These bounded native entries
// let a lane complete only the missing part of a capped matrix process.
#[test]
fn capture_span_cranelift_context_and_retention() {
    check_capture(
        Some(native::Backend::Cranelift),
        &["capture_context_and_retention"],
    );
}
#[test]
fn capture_span_cranelift_expand_partial() {
    check_capture(
        Some(native::Backend::Cranelift),
        &["capture_expand_partial"],
    );
}
#[test]
fn capture_span_cranelift_forced_limits() {
    check_capture(Some(native::Backend::Cranelift), &["capture_forced_limits"]);
}
#[test]
fn capture_span_llvm_goldens() {
    check_capture(Some(native::Backend::Llvm), &["capture_goldens"]);
}
#[test]
fn capture_span_llvm_context_and_retention() {
    check_capture(
        Some(native::Backend::Llvm),
        &["capture_context_and_retention"],
    );
}
#[test]
fn capture_span_llvm_expand_partial() {
    check_capture(Some(native::Backend::Llvm), &["capture_expand_partial"]);
}
#[test]
fn capture_span_llvm_forced_limits() {
    check_capture(Some(native::Backend::Llvm), &["capture_forced_limits"]);
}
