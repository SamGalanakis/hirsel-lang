//! Private engine differential checks run without exporting selectors from std.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::{Path, PathBuf};

fn private_fixture(directory: &Path, interpreter: bool) -> PathBuf {
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
        "path": root.join("crates/hirsel-driver/tests/fixtures/regex-dfa-private.hir").canonicalize().unwrap(),
        "module": ["regex"]
    }));
    manifest["entry"] = serde_json::json!({"module": ["regex"], "name": "dfa_test_main"});
    if interpreter {
        let fixture = std::fs::read_to_string(
            root.join("crates/hirsel-driver/tests/fixtures/regex-dfa-private.hir"),
        )
        .unwrap();
        std::fs::write(
            directory.join("interpreter.hir"),
            interpreter_driver(&fixture),
        )
        .unwrap();
        manifest["sources"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "path": directory.join("interpreter.hir"), "module": ["regex"]
            }));
    }
    let path = directory.join("hirsel.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    path
}

fn private_check(backend: Option<native::Backend>) {
    with_compiler_stack(|| {
        let temp = tempfile::tempdir().unwrap();
        let path = private_fixture(temp.path(), backend.is_none());
        let run = match backend {
            None => {
                let mut manifest: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                for entry in [
                    "dfa_interpreter_matches",
                    "dfa_interpreter_misc",
                    "dfa_interpreter_work_0",
                    "dfa_interpreter_work_1",
                    "dfa_interpreter_work_2",
                    "dfa_interpreter_work_3",
                ] {
                    manifest["entry"]["name"] = entry.into();
                    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
                    let outcome = run_path_with_step_budget(
                        &path,
                        SimulationConfig::default(),
                        Some(200_000_000),
                        Verify::On,
                        &mut Vec::new(),
                        &mut Vec::new(),
                    )
                    .unwrap();
                    assert!(
                        matches!(
                            outcome,
                            RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                        ),
                        "{entry}: {outcome:?}"
                    );
                }
                return;
            }
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
            matches!(run, RunOutcome::Returned(hirsel_run_schema::Returned::Ok)),
            "{backend:?}: {run:?}"
        );
    });
}

#[test]
fn regex_dfa_private_interpreter() {
    private_check(None);
}

#[test]
fn regex_dfa_private_llvm() {
    private_check(Some(native::Backend::Llvm));
}

#[test]
fn regex_dfa_private_cranelift() {
    private_check(Some(native::Backend::Cranelift));
}

#[test]
fn regex_dfa_corpus_llvm() {
    with_compiler_stack(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut names = vec![
            "SEM-3-regex-all-5-extract-run.hir".to_owned(),
            "SEM-3-regex-all-5-limits-run.hir".to_owned(),
            "SEM-3-regex-edges-run.hir".to_owned(),
            "SEM-3-regex-limits-run.hir".to_owned(),
            "SEM-3-regex-go-1-run.hir".to_owned(),
            "SEM-3-regex-go-2-run.hir".to_owned(),
            "SEM-3-regex-go-3-run.hir".to_owned(),
            "SEM-3-regex-captures-captures-11-run.hir".to_owned(),
            "SEM-3-regex-captures-replacement-9-part-4-run.hir".to_owned(),
            "SEM-3-regex-captures-replacement-9-part-5-run.hir".to_owned(),
        ];
        names.extend((1..=28).map(|i| format!("SEM-3-regex-all-5-go-{i:03}-run.hir")));
        for name in names {
            let path = root.join("conformance/SEM").join(&name);
            let run = native::run_path_native_backend(
                &path,
                HostLaunch::empty(),
                None,
                native::Backend::Llvm,
                native::Output::Streams {
                    stdout: &mut Vec::new(),
                    stderr: &mut Vec::new(),
                },
                Verify::On,
            )
            .unwrap()
            .unwrap();
            assert!(
                matches!(run, RunOutcome::Returned(hirsel_run_schema::Returned::Ok)),
                "{name}: {run:?}"
            );
        }
    });
}

// Native runs retain the complete original fixture. Give each disjoint
// interpreter section the existing tooling budget; do not discard assertions
// or increase the aggregate-run cap after its recorded Steps exhaustion.
fn interpreter_driver(source: &str) -> String {
    let main = &source[source.find("pub fn dfa_test_main()").unwrap()
        ..source.find("// Fill the forward cache").unwrap()];
    let head = main.split("  if !dfa_public_sweeps()").next().unwrap();
    let mut driver = head.replace("dfa_test_main()", "dfa_interpreter_matches()");
    driver.push_str("  Ok(())\n}\n");
    driver.push_str("pub fn dfa_interpreter_misc() -> Result[(), DfaTestFailure] uses {} {\n");
    driver.push_str(&main[main.find("  if !dfa_scalar_retry()").unwrap()..]);
    let sweep = &source[source.find("fn dfa_public_sweeps()").unwrap()..];
    let sweep = &sweep[..sweep.find("\n}").unwrap() + 2];
    let declaration = sweep
        .lines()
        .find(|line| line.contains("let patterns:"))
        .unwrap();
    let patterns: Vec<String> =
        serde_json::from_str(declaration.split_once(" = ").unwrap().1).unwrap();
    assert_eq!(patterns.len(), 13);
    driver.push_str(
        &sweep
            .replace(
                "fn dfa_public_sweeps()",
                "fn dfa_interpreter_sweeps(patterns: List[String])",
            )
            .replace(&format!("{declaration}\n"), ""),
    );
    driver.push('\n');
    for (index, chunk) in patterns.chunks(4).enumerate() {
        let list = serde_json::to_string(chunk).unwrap();
        driver.push_str(&format!("pub fn dfa_interpreter_work_{index}() -> Result[(), DfaTestFailure] uses {{}} {{\n let patterns: List[String] = {list}\n if dfa_interpreter_sweeps(patterns) {{ Ok(()) }} else {{ Error(DfaTestFailure(pattern: \"partition\", src: \"aaa/64a\", stage: {index})) }}\n}}\n"));
    }
    assert_eq!(driver.matches("if !dfa_scalar_retry()").count(), 1);
    assert_eq!(driver.matches("pub fn dfa_interpreter_").count(), 6);
    assert!(!driver.contains("fn dfa_scalar_retry()"));
    driver
}
