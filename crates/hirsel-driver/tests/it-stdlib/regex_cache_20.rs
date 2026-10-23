//! Private engine differential checks run without exporting selectors from std.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::{Path, PathBuf};

fn private_fixture(directory: &Path) -> PathBuf {
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
    std::fs::write(directory.join("cache.hir"), CACHE_VECTORS).unwrap();
    manifest["sources"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": directory.join("cache.hir"), "module": ["regex"]
        }));
    manifest["entry"] = serde_json::json!({"module": ["regex"], "name": "cache_small"});
    let path = directory.join("hirsel.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    path
}

fn check_entry(engine: u8, entry: &str, additional: &str) {
    with_compiler_stack(|| {
        let temp = tempfile::tempdir().unwrap();
        let path = private_fixture(temp.path());
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if !additional.is_empty() {
            std::fs::write(
                temp.path().join("cache.hir"),
                format!("{CACHE_VECTORS}\n{additional}"),
            )
            .unwrap();
        }
        manifest["entry"]["name"] = entry.into();
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        if engine == 1 {
            let mut loaded = load_program(&path).unwrap();
            loaded.check_for_execution().unwrap();
            let input = loaded.engine.export_native_input().unwrap();
            let validated = loaded.engine.import_native_input(input.clone()).unwrap();
            let ready = native::build(
                &validated,
                &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
            )
            .unwrap();
            let run = native::run_heap_in(
                &ready,
                &loaded.entry,
                &native::Sites::new(&input),
                &native::HeapWorld {
                    steps: Some(100_000_000),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(run.observation.exit_code, Some(0));
        } else {
            let outcome = if engine == 0 {
                run_path_with_step_budget(
                    &path,
                    SimulationConfig::default(),
                    Some(100_000_000),
                    Verify::On,
                    &mut Vec::new(),
                    &mut Vec::new(),
                )
                .unwrap()
            } else {
                native::run_path_native_backend(
                    &path,
                    HostLaunch::empty(),
                    None,
                    if engine == 2 {
                        native::Backend::Cranelift
                    } else {
                        native::Backend::Llvm
                    },
                    native::Output::Streams {
                        stdout: &mut Vec::new(),
                        stderr: &mut Vec::new(),
                    },
                    Verify::On,
                )
                .unwrap()
                .unwrap()
            };
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{engine} {entry}: {outcome:?}"
            );
        }
    });
}

#[test]
fn cache_20_interpreter() {
    for (entry, source) in partitions().into_iter().skip(5) {
        eprintln!("interpreter {entry}");
        check_entry(0, &entry, &source);
    }
}
#[test]
fn cache_20_interpreter_vectors() {
    for (entry, source) in partitions().into_iter().take(5) {
        check_entry(0, &entry, &source);
    }
}
#[test]
fn cache_20_checked_h() {
    let (entry, source) = partitions().pop().unwrap();
    check_entry(1, &entry, &source);
}
#[test]
fn cache_20_cranelift() {
    check_entry(2, "cache_small", "");
}
#[test]
fn cache_20_llvm() {
    check_entry(3, "cache_small", "");
}
#[test]
fn cache_20_large_cranelift() {
    check_entry(2, "cache_large", "");
}
#[test]
fn cache_20_large_llvm() {
    check_entry(3, "cache_large", "");
}

#[test]
fn cache_20_reverse_only_quit() {
    for engine in 0..4 {
        check_entry(engine, "reverse_probe", "");
    }
}

const CACHE_VECTORS: &str = include_str!("../fixtures/regex-cache-20.hir");

// Preserve every assertion from the native aggregate while giving each
// interpreter partition the unchanged per-program tooling budget.
fn partitions() -> Vec<(String, String)> {
    let body = CACHE_VECTORS.split_once("pub fn cache_small()").unwrap().1;
    let body = body.split_once("pub fn cache_large()").unwrap().0;
    let vectors =
        &body[body.find("  let vectors:").unwrap()..body.find("  let log_patterns:").unwrap()];
    let logs = &body[body.find("  let log_patterns:").unwrap()
        ..body.find("  if !cache_case(\"\\\\{\\\\{").unwrap()];
    let template = &body[body.find("  if !cache_case(\"\\\\{\\\\{").unwrap()
        ..body.find("  // Deterministic randomized").unwrap()];
    let random =
        &body[body.find("  let atoms:").unwrap()..body.find("  // Force repeated").unwrap()];
    let policy = &body[body.find("  // Force repeated").unwrap()..];
    let mut out = Vec::new();
    let mut push = |name: String, body: String| {
        let source = format!(
            "pub fn {name}() -> Result[(), CacheFailure] uses {{}} {{\n{body}\n Ok(())\n}}\n"
        );
        out.push((name, source));
    };
    for index in 0..4 {
        let selected = vectors.replace(
            "for (pattern, src) in vectors {",
            &format!(
                "for n in {}u64..{} {{\n let (pattern, src) = vectors[n]",
                index * 4,
                index * 4 + 4
            ),
        );
        push(format!("cache_vectors_{index}"), selected);
    }
    let window = &CACHE_VECTORS[CACHE_VECTORS.find("fn cache_case(").unwrap()
        ..CACHE_VECTORS.find("pub fn cache_small()").unwrap()];
    let window = window
        .replace("fn cache_case(", "fn cache_case_window(")
        .replace("capacity: u64)", "capacity: u64, first: u64, last: u64)")
        .replace("let mut start = 0u64", "let mut start = first")
        .replace(
            "while start <= core@1/list.len(subject.scalars)",
            "while start <= core@1/list.len(subject.scalars) && start < last",
        );
    for index in 0..13 {
        let selected = logs.replace(
            "for pattern in log_patterns {",
            &format!(
                "for n in {index}u64..{} {{\n let pattern = log_patterns[n]",
                index + 1
            ),
        );
        if [1, 2, 6, 7].contains(&index) {
            // Original 134-byte subject, every boundary including EOF, in
            // fixed disjoint windows. No assertion or input is omitted.
            for from in (0..135).step_by(16) {
                let name = format!("cache_log_{index}_{from}");
                let body = selected
                    .replace(", 65024)", &format!(", 65024, {from}, {})", from + 16))
                    .replace("cache_case(", "cache_case_window(");
                push(name, body);
                // The helper is appended below after the construction closure
                // releases its borrow of the result vector.
            }
        } else {
            push(format!("cache_log_{index}"), selected);
        }
    }
    push("cache_template".into(), template.into());
    for index in 0..6 {
        let selected = random.replace("for sample in 0u64..48 {", &format!("for skip in 0u64..{} {{ random = (random * 1664525 + 1013904223) % 4294967296 }}\n for sample in 0u64..8 {{", index * 8 * 7));
        push(format!("cache_random_{index}"), selected);
    }
    // The policy follows 48 samples with seven PRNG advances each.
    let policy = policy.strip_suffix("\n\n").unwrap_or(policy);
    let policy = policy.rsplit_once("  Ok(())\n}").unwrap().0;
    push(
        "cache_policy".into(),
        format!(
            "let mut random = 20261023u64\n for skip in 0u64..336 {{ random = (random * 1664525 + 1013904223) % 4294967296 }}\n{policy}"
        ),
    );
    assert_eq!(out.len(), 57);
    for (name, source) in &mut out {
        if name.starts_with("cache_log_") && name.matches('_').count() == 3 {
            *source = format!("{window}\n{source}");
        }
    }
    out
}
